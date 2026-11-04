//! Image rendering — Vulkan pipeline for textured quads.
//!
//! Mirrors `image.c` / `image.h` from the C source.
//!
//! Each call to [`ImageRenderer::prepare_image`] looks up (or loads) a texture
//! by name and records a draw quad.  Up to [`MAX_CACHED_IMAGES`] textures
//! are kept in an LRU cache; the least-recently-used entry is evicted when the
//! cache is full.  All quads are batched and drawn in a single `draw_images`
//! call at the end of each frame.
//!
//! A name is either a filesystem path or an `asset:<provider>/<file>` URI naming a
//! provider-scoped asset — compiled into the binary for a built-in, or a file in a
//! plugin's own install directory. See [`decode_image_source`] and
//! `sicompass_sdk::assets`. Either way the cache is keyed by the string, so one
//! name is one texture.

use crate::app_state::SiError;
use crate::render;
use crate::shaders;
use ash::vk;
use std::ptr;

// Use the `image` crate under an alias to avoid shadowing this module's name.
use ::image as img_crate;
use img_crate::GenericImageView as _;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How many textures stay on the GPU. Thumbnails are small, so a screen of an album fits
/// with room to scroll back.
const MAX_CACHED_IMAGES: usize = 96;
/// How many pictures one frame can draw, which the vertex buffer is sized for.
const MAX_DRAWS: usize = 160;
const VERTS_PER_QUAD: usize = 6;
const MAX_IMAGE_VERTICES: usize = MAX_DRAWS * VERTS_PER_QUAD;
/// Textures uploaded to the GPU in one frame. Decoding is on other threads, but the
/// upload is not, so a screen of photos arrives over a few frames instead of stalling one.
const UPLOADS_PER_FRAME: usize = 4;

// ---------------------------------------------------------------------------
// Vertex layout (must match shaders/image_vert.spv)
// ---------------------------------------------------------------------------

/// Per-vertex data: screen-space position + UV texture coordinate.
#[repr(C)]
struct ImageVertex {
    pos: [f32; 2],
    tex_coord: [f32; 2],
}

// ---------------------------------------------------------------------------
// Cache entry
// ---------------------------------------------------------------------------

pub(crate) struct CachedTexture {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    sampler: vk::Sampler,
    path: String,
    /// The longest side this texture was made for, or 0 for one made at full size.
    class: u32,
    width: u32,
    height: u32,
    last_used_frame: u64,
}

// ---------------------------------------------------------------------------
// Per-frame draw record
// ---------------------------------------------------------------------------

struct ImageDraw {
    slot: usize,
    vertex_offset: u32,
}

// ---------------------------------------------------------------------------
// ImageRenderer
// ---------------------------------------------------------------------------

pub struct ImageRenderer {
    // Vulkan context (cloned handles — ash wraps in Arc internally)
    device: ash::Device,
    instance: ash::Instance,
    physical_device: vk::PhysicalDevice,
    command_pool: vk::CommandPool,
    queue: vk::Queue,

    // GPU resources
    vertex_buffer: vk::Buffer,
    vertex_buffer_memory: vk::DeviceMemory,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set_layout: vk::DescriptorSetLayout,
    /// Pre-allocated descriptor sets — one per cache slot.
    descriptor_sets: Vec<vk::DescriptorSet>,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,

    // Texture cache: MAX_CACHED_IMAGES slots, None = empty.
    cache: Vec<Option<CachedTexture>>,

    /// Decodes pictures on other threads, see [`Loader`].
    loader: Loader,
    /// Pictures that could not be decoded, not asked for again.
    broken: std::collections::HashSet<String>,

    // Per-frame CPU accumulators
    draws: Vec<ImageDraw>,
    vertices: Vec<ImageVertex>,
    current_frame: u64,
}

impl ImageRenderer {
    pub unsafe fn new(
        device: &ash::Device,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        command_pool: vk::CommandPool,
        queue: vk::Queue,
        render_pass: vk::RenderPass,
    ) -> Result<Self, SiError> {
        unsafe {
            // ---- Vertex buffer (host-visible, host-coherent) ----------------------
            let vb_size =
                (std::mem::size_of::<ImageVertex>() * MAX_IMAGE_VERTICES) as vk::DeviceSize;
            let (vertex_buffer, vertex_buffer_memory) = render::create_buffer(
                device,
                instance,
                physical_device,
                vb_size,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )?;

            // ---- Descriptor set layout -------------------------------------------
            let sampler_binding = vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT);
            let dsl_info = vk::DescriptorSetLayoutCreateInfo::default()
                .bindings(std::slice::from_ref(&sampler_binding));
            let descriptor_set_layout = device.create_descriptor_set_layout(&dsl_info, None)?;

            // ---- Descriptor pool (one set per cache slot) -------------------------
            let pool_size = vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(MAX_CACHED_IMAGES as u32);
            let pool_info = vk::DescriptorPoolCreateInfo::default()
                .max_sets(MAX_CACHED_IMAGES as u32)
                .pool_sizes(std::slice::from_ref(&pool_size));
            let descriptor_pool = device.create_descriptor_pool(&pool_info, None)?;

            // Allocate all sets upfront
            let layouts = vec![descriptor_set_layout; MAX_CACHED_IMAGES];
            let alloc_info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(descriptor_pool)
                .set_layouts(&layouts);
            let descriptor_sets = device.allocate_descriptor_sets(&alloc_info)?;

            // ---- Pipeline --------------------------------------------------------
            let vert_module = render::create_shader_module(device, shaders::IMAGE_VERT)?;
            let frag_module = render::create_shader_module(device, shaders::IMAGE_FRAG)?;

            let entry = std::ffi::CString::new("main").unwrap();
            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(vert_module)
                    .name(&entry),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(frag_module)
                    .name(&entry),
            ];

            let stride = std::mem::size_of::<ImageVertex>() as u32;
            let binding_desc = vk::VertexInputBindingDescription::default()
                .binding(0)
                .stride(stride)
                .input_rate(vk::VertexInputRate::VERTEX);
            // pos@0(8B), texCoord@8(8B)
            let attr_descs = [
                vk::VertexInputAttributeDescription::default()
                    .location(0)
                    .binding(0)
                    .format(vk::Format::R32G32_SFLOAT)
                    .offset(0),
                vk::VertexInputAttributeDescription::default()
                    .location(1)
                    .binding(0)
                    .format(vk::Format::R32G32_SFLOAT)
                    .offset(8),
            ];

            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(std::slice::from_ref(&binding_desc))
                .vertex_attribute_descriptions(&attr_descs);

            let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

            let viewport_state = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);

            let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .line_width(1.0)
                .cull_mode(vk::CullModeFlags::NONE)
                .front_face(vk::FrontFace::COUNTER_CLOCKWISE);

            let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);

            let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(false)
                .depth_write_enable(false)
                .depth_compare_op(vk::CompareOp::ALWAYS);

            let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(
                    vk::ColorComponentFlags::R
                        | vk::ColorComponentFlags::G
                        | vk::ColorComponentFlags::B
                        | vk::ColorComponentFlags::A,
                )
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
                .alpha_blend_op(vk::BlendOp::ADD);

            let blend_state = vk::PipelineColorBlendStateCreateInfo::default()
                .attachments(std::slice::from_ref(&blend_attachment));

            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic_state =
                vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

            // Push constant: screenWidth, screenHeight (vec2 in vertex shader)
            let push_range = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(std::mem::size_of::<[f32; 2]>() as u32);

            let set_layouts = [descriptor_set_layout];
            let pl_info = vk::PipelineLayoutCreateInfo::default()
                .set_layouts(&set_layouts)
                .push_constant_ranges(std::slice::from_ref(&push_range));
            let pipeline_layout = device.create_pipeline_layout(&pl_info, None)?;

            let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&input_assembly)
                .viewport_state(&viewport_state)
                .rasterization_state(&rasterizer)
                .multisample_state(&multisampling)
                .depth_stencil_state(&depth_stencil)
                .color_blend_state(&blend_state)
                .dynamic_state(&dynamic_state)
                .layout(pipeline_layout)
                .render_pass(render_pass)
                .subpass(0);

            let pipeline = device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
                .map_err(|(_, e)| SiError::Vulkan(e))?[0];

            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);

            Ok(ImageRenderer {
                device: device.clone(),
                instance: instance.clone(),
                physical_device,
                command_pool,
                queue,
                vertex_buffer,
                vertex_buffer_memory,
                descriptor_pool,
                descriptor_set_layout,
                descriptor_sets,
                pipeline_layout,
                pipeline,
                loader: Loader::new(3),
                broken: std::collections::HashSet::new(),
                cache: (0..MAX_CACHED_IMAGES).map(|_| None).collect(),
                draws: Vec::new(),
                vertices: Vec::with_capacity(MAX_IMAGE_VERTICES),
                current_frame: 0,
            })
        }
    }

    // ---- Frame helpers -------------------------------------------------------

    /// Reset per-frame draw list. Call once at the start of each frame.
    pub fn begin_image_rendering(&mut self) {
        self.draws.clear();
        self.vertices.clear();
        self.current_frame += 1;
    }

    /// Return `(width, height)` of the cached/loaded texture, or `None` on failure.
    pub unsafe fn texture_size(&mut self, path: &str) -> Option<(u32, u32)> {
        unsafe {
            let slot = self.find_or_load(path)?;
            let tex = self.cache[slot].as_ref()?;
            Some((tex.width, tex.height))
        }
    }

    /// Schedule a textured quad at (x, y, w, h).
    ///
    /// If `path` is already cached the texture is reused; otherwise the image
    /// file is loaded, uploaded to the GPU, and placed in the least-recently-
    /// used cache slot.
    pub unsafe fn prepare_image(&mut self, path: &str, x: f32, y: f32, width: f32, height: f32) {
        unsafe {
            if self.draws.len() >= MAX_DRAWS {
                return;
            }

            let slot = match self.find_or_load(path) {
                Some(s) => s,
                None => return,
            };

            let vertex_offset = self.vertices.len() as u32;

            let (x0, y0, x1, y1) = (x, y, x + width, y + height);
            // Two triangles covering the quad, UV (0,0)→(1,1)
            self.vertices.push(ImageVertex {
                pos: [x0, y0],
                tex_coord: [0.0, 0.0],
            });
            self.vertices.push(ImageVertex {
                pos: [x1, y0],
                tex_coord: [1.0, 0.0],
            });
            self.vertices.push(ImageVertex {
                pos: [x1, y1],
                tex_coord: [1.0, 1.0],
            });
            self.vertices.push(ImageVertex {
                pos: [x0, y0],
                tex_coord: [0.0, 0.0],
            });
            self.vertices.push(ImageVertex {
                pos: [x1, y1],
                tex_coord: [1.0, 1.0],
            });
            self.vertices.push(ImageVertex {
                pos: [x0, y1],
                tex_coord: [0.0, 1.0],
            });

            self.draws.push(ImageDraw {
                slot,
                vertex_offset,
            });
        }
    }

    /// Like `prepare_image`, but clips the quad vertically to the screen range
    /// `[clip_top, clip_bottom]`, cropping the texture (adjusting UVs) rather
    /// than stretching it. Used by scroll mode so a scrolled image does not
    /// bleed over the header / tabs band.
    pub unsafe fn prepare_image_clipped(
        &mut self,
        path: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        clip_top: f32,
        clip_bottom: f32,
    ) {
        unsafe {
            if self.draws.len() >= MAX_DRAWS {
                return;
            }
            if height <= 0.0 {
                return;
            }

            let vis_top = y.max(clip_top);
            let vis_bottom = (y + height).min(clip_bottom);
            if vis_bottom <= vis_top {
                return;
            } // fully clipped away

            let slot = match self.find_or_load(path) {
                Some(s) => s,
                None => return,
            };

            let vertex_offset = self.vertices.len() as u32;

            // Crop UVs to the visible vertical band.
            let v0 = ((vis_top - y) / height).clamp(0.0, 1.0);
            let v1 = ((vis_bottom - y) / height).clamp(0.0, 1.0);
            let (x0, y0, x1, y1) = (x, vis_top, x + width, vis_bottom);
            self.vertices.push(ImageVertex {
                pos: [x0, y0],
                tex_coord: [0.0, v0],
            });
            self.vertices.push(ImageVertex {
                pos: [x1, y0],
                tex_coord: [1.0, v0],
            });
            self.vertices.push(ImageVertex {
                pos: [x1, y1],
                tex_coord: [1.0, v1],
            });
            self.vertices.push(ImageVertex {
                pos: [x0, y0],
                tex_coord: [0.0, v0],
            });
            self.vertices.push(ImageVertex {
                pos: [x1, y1],
                tex_coord: [1.0, v1],
            });
            self.vertices.push(ImageVertex {
                pos: [x0, y1],
                tex_coord: [0.0, v1],
            });

            self.draws.push(ImageDraw {
                slot,
                vertex_offset,
            });
        }
    }

    /// Upload vertices and issue one draw call per queued image.
    pub unsafe fn draw_images(
        &self,
        device: &ash::Device,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
    ) {
        unsafe {
            if self.draws.is_empty() {
                return;
            }

            let upload_size =
                (std::mem::size_of::<ImageVertex>() * self.vertices.len()) as vk::DeviceSize;
            let ptr = device
                .map_memory(
                    self.vertex_buffer_memory,
                    0,
                    upload_size,
                    vk::MemoryMapFlags::empty(),
                )
                .unwrap() as *mut ImageVertex;
            ptr::copy_nonoverlapping(self.vertices.as_ptr(), ptr, self.vertices.len());
            device.unmap_memory(self.vertex_buffer_memory);

            let screen = [extent.width as f32, extent.height as f32];
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_push_constants(
                cb,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                std::slice::from_raw_parts(screen.as_ptr() as *const u8, 8),
            );

            let bufs = [self.vertex_buffer];
            let offs = [0u64];
            device.cmd_bind_vertex_buffers(cb, 0, &bufs, &offs);

            for draw in &self.draws {
                device.cmd_bind_descriptor_sets(
                    cb,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    0,
                    &[self.descriptor_sets[draw.slot]],
                    &[],
                );
                device.cmd_draw(cb, VERTS_PER_QUAD as u32, 1, draw.vertex_offset, 0);
            }
        }
    }

    // ---- Cleanup -------------------------------------------------------------

    pub unsafe fn cleanup(&mut self) {
        unsafe {
            for slot in self.cache.iter_mut() {
                if let Some(tex) = slot.take() {
                    self.device.destroy_sampler(tex.sampler, None);
                    self.device.destroy_image_view(tex.view, None);
                    self.device.destroy_image(tex.image, None);
                    self.device.free_memory(tex.memory, None);
                }
            }
            self.device.destroy_pipeline(self.pipeline, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            self.device.destroy_buffer(self.vertex_buffer, None);
            self.device.free_memory(self.vertex_buffer_memory, None);
        }
    }

    // ---- Internal helpers ----------------------------------------------------

    /// Return the cache slot index for `path`, loading the texture if needed.
    unsafe fn find_or_load(&mut self, path: &str) -> Option<usize> {
        unsafe {
            // Check if already cached
            for (i, slot) in self.cache.iter_mut().enumerate() {
                if let Some(tex) = slot {
                    if tex.path == path && tex.class == 0 {
                        tex.last_used_frame = self.current_frame;
                        return Some(i);
                    }
                }
            }

            // Find a free slot or the LRU slot
            let evict_slot = self.find_evict_slot();

            // Evict existing texture if necessary
            if let Some(old) = self.cache[evict_slot].take() {
                self.device.destroy_sampler(old.sampler, None);
                self.device.destroy_image_view(old.view, None);
                self.device.destroy_image(old.image, None);
                self.device.free_memory(old.memory, None);
            }

            // Load new texture
            match self.load_texture(path) {
                Ok(tex) => {
                    // Update the pre-allocated descriptor set for this slot
                    let image_info = vk::DescriptorImageInfo::default()
                        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                        .image_view(tex.view)
                        .sampler(tex.sampler);
                    let write = vk::WriteDescriptorSet::default()
                        .dst_set(self.descriptor_sets[evict_slot])
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(std::slice::from_ref(&image_info));
                    self.device.update_descriptor_sets(&[write], &[]);

                    self.cache[evict_slot] = Some(tex);
                    Some(evict_slot)
                }
                Err(e) => {
                    eprintln!("sicompass: image load failed for '{path}': {e}");
                    None
                }
            }
        }
    }

    /// Draw `path` into the box `bx` (x, y, w, h), as `fit` says, cropped to the screen
    /// rows `[clip_top, clip_bottom]`. Returns whether a picture was drawn: when it is
    /// still being decoded the caller draws a placeholder, and the picture arrives by
    /// itself.
    ///
    /// The texture is made no bigger than the box needs, so a screen of thumbnails is a
    /// few megabytes and a photo of twelve megapixels is read once, off this thread.
    pub unsafe fn prepare_picture(
        &mut self,
        path: &str,
        bx: [f32; 4],
        fit: crate::style::ObjectFit,
        clip_top: f32,
        clip_bottom: f32,
    ) -> bool {
        unsafe {
            if self.draws.len() >= MAX_DRAWS || self.broken.contains(path) {
                return false;
            }
            let wanted = size_class_for(bx[2].max(bx[3]));
            // The smallest texture that is big enough, else the biggest there is,
            // which shows blurred until the right one arrives.
            let (mut enough, mut biggest): (Option<(usize, u32)>, Option<(usize, u32)>) =
                (None, None);
            for (i, slot) in self.cache.iter().enumerate() {
                let Some(tex) = slot else { continue };
                if tex.path != path || tex.class == 0 {
                    continue;
                }
                if tex.class >= wanted && enough.is_none_or(|(_, class)| tex.class < class) {
                    enough = Some((i, tex.class));
                }
                if biggest.is_none_or(|(_, class)| tex.class > class) {
                    biggest = Some((i, tex.class));
                }
            }
            let best = enough.or(biggest);
            if best.is_none_or(|(_, class)| class < wanted) {
                self.loader.request(path, wanted);
            }
            let Some((slot, _)) = best else { return false };
            let Some(tex) = self.cache[slot].as_mut() else {
                return false;
            };
            tex.last_used_frame = self.current_frame;
            let (tw, th) = (tex.width, tex.height);
            let Some(place) = place_picture(bx, (tw, th), fit, clip_top, clip_bottom) else {
                // Nothing of it shows, which counts as drawn.
                return true;
            };
            let vertex_offset = self.vertices.len() as u32;
            let [x0, y0, x1, y1] = place.rect;
            let [u0, v0, u1, v1] = place.uv;
            for (pos, tex_coord) in [
                ([x0, y0], [u0, v0]),
                ([x1, y0], [u1, v0]),
                ([x1, y1], [u1, v1]),
                ([x0, y0], [u0, v0]),
                ([x1, y1], [u1, v1]),
                ([x0, y1], [u0, v1]),
            ] {
                self.vertices.push(ImageVertex { pos, tex_coord });
            }
            self.draws.push(ImageDraw {
                slot,
                vertex_offset,
            });
            true
        }
    }

    /// Upload what the decoder threads have finished, a few at a time. Returns whether
    /// there is more to come, so the caller keeps redrawing until the pictures are in.
    pub unsafe fn pump_loads(&mut self) -> bool {
        unsafe {
            for done in self.loader.poll(UPLOADS_PER_FRAME) {
                let Some((width, height, pixels)) = done.image else {
                    self.broken.insert(done.path);
                    continue;
                };
                // Never over a texture this frame already draws.
                let Some(slot) = self.find_free_slot_for_upload() else {
                    continue;
                };
                if let Some(old) = self.cache[slot].take() {
                    self.destroy_texture(old);
                }
                match self.upload_rgba(&done.path, done.class, width, height, &pixels) {
                    Ok(tex) => {
                        self.bind_slot(slot, &tex);
                        self.cache[slot] = Some(tex);
                    }
                    Err(e) => {
                        eprintln!("sicompass: image upload failed for '{}': {e}", done.path);
                        self.broken.insert(done.path);
                    }
                }
            }
            self.loader.pending() > 0
        }
    }

    /// A slot to upload into: an empty one, else the least recently used that this
    /// frame is not drawing.
    fn find_free_slot_for_upload(&self) -> Option<usize> {
        let slot = find_evict_slot_in(&self.cache);
        let in_use = self.cache[slot]
            .as_ref()
            .is_some_and(|t| t.last_used_frame >= self.current_frame);
        (!in_use).then_some(slot)
    }

    unsafe fn destroy_texture(&self, old: CachedTexture) {
        unsafe {
            self.device.destroy_sampler(old.sampler, None);
            self.device.destroy_image_view(old.view, None);
            self.device.destroy_image(old.image, None);
            self.device.free_memory(old.memory, None);
        }
    }

    /// Point a slot's descriptor set at a texture.
    unsafe fn bind_slot(&self, slot: usize, tex: &CachedTexture) {
        unsafe {
            let image_info = vk::DescriptorImageInfo::default()
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image_view(tex.view)
                .sampler(tex.sampler);
            let write = vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_sets[slot])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(std::slice::from_ref(&image_info));
            self.device.update_descriptor_sets(&[write], &[]);
        }
    }

    /// Choose the slot to evict: prefer empty slots, then pick the LRU.
    fn find_evict_slot(&self) -> usize {
        find_evict_slot_in(&self.cache)
    }

    /// Decode an image and upload it to a new `VkImage`.
    unsafe fn load_texture(&self, path: &str) -> Result<CachedTexture, SiError> {
        unsafe {
            let dyn_img = decode_image_source(path)?;
            let (width, height) = dyn_img.dimensions();
            let rgba = dyn_img.into_rgba8();
            self.upload_rgba(path, 0, width, height, &rgba)
        }
    }

    /// Put decoded pixels on the GPU.
    unsafe fn upload_rgba(
        &self,
        path: &str,
        class: u32,
        width: u32,
        height: u32,
        pixel_bytes: &[u8],
    ) -> Result<CachedTexture, SiError> {
        unsafe {
            // Staging buffer
            let buf_size = pixel_bytes.len() as vk::DeviceSize;
            let (staging_buf, staging_mem) = render::create_buffer(
                &self.device,
                &self.instance,
                self.physical_device,
                buf_size,
                vk::BufferUsageFlags::TRANSFER_SRC,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )?;
            let ptr =
                self.device
                    .map_memory(staging_mem, 0, buf_size, vk::MemoryMapFlags::empty())?
                    as *mut u8;
            ptr::copy_nonoverlapping(pixel_bytes.as_ptr(), ptr, pixel_bytes.len());
            self.device.unmap_memory(staging_mem);

            // Device-local VkImage (R8G8B8A8_SRGB)
            let (image, memory) = render::create_image_helper(
                &self.device,
                &self.instance,
                self.physical_device,
                width,
                height,
                vk::Format::R8G8B8A8_SRGB,
                vk::ImageTiling::OPTIMAL,
                vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )?;

            // Transition → copy → transition
            render::transition_image_layout(
                &self.device,
                self.command_pool,
                self.queue,
                image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            );
            render::copy_buffer_to_image(
                &self.device,
                self.command_pool,
                self.queue,
                staging_buf,
                image,
                width,
                height,
            );
            render::transition_image_layout(
                &self.device,
                self.command_pool,
                self.queue,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );

            // Staging buffer no longer needed
            self.device.destroy_buffer(staging_buf, None);
            self.device.free_memory(staging_mem, None);

            // Image view
            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_SRGB)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(0)
                        .level_count(1)
                        .base_array_layer(0)
                        .layer_count(1),
                );
            let view = self.device.create_image_view(&view_info, None)?;

            // Sampler (linear filter, clamp to edge)
            let sampler_info = vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .anisotropy_enable(false)
                .unnormalized_coordinates(false);
            let sampler = self.device.create_sampler(&sampler_info, None)?;

            Ok(CachedTexture {
                image,
                memory,
                view,
                sampler,
                path: path.to_owned(),
                class,
                width,
                height,
                last_used_frame: self.current_frame,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Standalone helpers (pub(crate) for testability)
// ---------------------------------------------------------------------------

/// Decode an image named either by an `asset:` URI or by a filesystem path.
///
/// Split out of `load_texture` so it needs no Vulkan device and can be tested.
///
/// `load_from_memory` sniffs the format from the magic bytes, while `open` tries
/// the extension first and falls back to the same sniffing. For PNG, JPEG and WebP
/// — everything shipped or plausibly shipped as an asset — the magic is
/// unambiguous, so the two agree.
pub(crate) fn decode_image_source(path: &str) -> Result<img_crate::DynamicImage, SiError> {
    if sicompass_sdk::assets::is_uri(path) {
        let bytes = sicompass_sdk::assets::resolve(path)
            .ok_or_else(|| SiError::Other(format!("no such asset: {path}")))?;
        return img_crate::load_from_memory(&bytes)
            .map_err(|e| SiError::Other(format!("image decode: {e}")));
    }
    img_crate::open(path).map_err(|e| SiError::Other(format!("image decode: {e}")))
}

// ---------------------------------------------------------------------------
// Sizes, placement and decoding, none of which needs a GPU
// ---------------------------------------------------------------------------

/// The pixel size of a picture, from its header and not its pixels, and with EXIF
/// orientation applied, so a portrait photo from a phone is tall. `None` if it cannot be
/// read. Kept, so laying out a thousand photos reads each header once.
pub fn dimensions(path: &str) -> Option<(u32, u32)> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static KNOWN: OnceLock<Mutex<HashMap<String, Option<(u32, u32)>>>> = OnceLock::new();
    let known = KNOWN.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(found) = known.lock().ok()?.get(path) {
        return *found;
    }
    let read = read_dimensions(path);
    let mut known = known.lock().ok()?;
    if known.len() > 20_000 {
        known.clear();
    }
    known.insert(path.to_owned(), read);
    read
}

fn reader_for(path: &str) -> Option<img_crate::ImageReader<std::io::Cursor<Vec<u8>>>> {
    // A file is read whole, not opened, so that this and the decoder see one thing
    // and the header read stays a single system call.
    let bytes = if sicompass_sdk::assets::is_uri(path) {
        sicompass_sdk::assets::resolve(path)?.to_vec()
    } else {
        std::fs::read(path).ok()?
    };
    img_crate::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
}

fn read_dimensions(path: &str) -> Option<(u32, u32)> {
    use img_crate::ImageDecoder as _;
    // Only the front of a big file is needed, so read just that for a path.
    let mut decoder = if sicompass_sdk::assets::is_uri(path) {
        reader_for(path)?.into_decoder().ok()?
    } else {
        let file = std::fs::File::open(path).ok()?;
        let reader = img_crate::ImageReader::new(std::io::BufReader::new(file))
            .with_guessed_format()
            .ok()?;
        return dimensions_of(reader.into_decoder().ok()?);
    };
    let (w, h) = decoder.dimensions();
    let turned = decoder.orientation().ok().is_some_and(orientation_swaps);
    Some(if turned { (h, w) } else { (w, h) })
}

fn dimensions_of(mut decoder: impl img_crate::ImageDecoder) -> Option<(u32, u32)> {
    let (w, h) = decoder.dimensions();
    let turned = decoder.orientation().ok().is_some_and(orientation_swaps);
    Some(if turned { (h, w) } else { (w, h) })
}

/// Whether an EXIF orientation turns the picture on its side.
fn orientation_swaps(o: img_crate::metadata::Orientation) -> bool {
    use img_crate::metadata::Orientation::*;
    matches!(o, Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH)
}

/// How big a texture a box of `side` pixels (its longer side) needs: a size class, so
/// boxes of nearly the same size share one texture. Doubled so a texture is never
/// stretched on a screen with a high pixel density, up to 2048.
pub(crate) fn size_class_for(side: f32) -> u32 {
    let need = side.max(1.0) * 1.5;
    [256, 512, 1024, 2048]
        .into_iter()
        .find(|class| need <= *class as f32)
        .unwrap_or(2048)
}

/// Decode a picture, turn it upright, and make it no bigger than `max_side` on its
/// longer side. Returns its size and RGBA pixels.
pub(crate) fn decode_scaled(path: &str, max_side: u32) -> Result<(u32, u32, Vec<u8>), SiError> {
    use img_crate::ImageDecoder as _;
    let mut decoder = reader_for(path)
        .ok_or_else(|| SiError::Other(format!("cannot read {path}")))?
        .into_decoder()
        .map_err(|e| SiError::Other(format!("image decode: {e}")))?;
    let orientation = decoder.orientation().ok();
    let mut picture = img_crate::DynamicImage::from_decoder(decoder)
        .map_err(|e| SiError::Other(format!("image decode: {e}")))?;
    if let Some(orientation) = orientation {
        picture.apply_orientation(orientation);
    }
    let (w, h) = picture.dimensions();
    if w.max(h) > max_side {
        picture = picture.thumbnail(max_side, max_side);
    }
    let (w, h) = picture.dimensions();
    Ok((w, h, picture.into_rgba8().into_raw()))
}

/// Where a picture goes inside its box, and which part of it shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placement {
    /// Left, top, right, bottom on the screen.
    pub rect: [f32; 4],
    /// The part of the picture shown, in 0 to 1.
    pub uv: [f32; 4],
}

/// Fit a picture of `image` pixels into the box `bx` (x, y, w, h). `Contain` shows all of
/// it, centred, `Cover` fills the box and crops what overflows. Rows outside
/// `[clip_top, clip_bottom]` are cut off and their part of the picture with them. `None`
/// when nothing shows.
pub(crate) fn place_picture(
    bx: [f32; 4],
    image: (u32, u32),
    fit: crate::style::ObjectFit,
    clip_top: f32,
    clip_bottom: f32,
) -> Option<Placement> {
    let [x, y, w, h] = bx;
    let (iw, ih) = (image.0 as f32, image.1 as f32);
    if w <= 0.0 || h <= 0.0 || iw <= 0.0 || ih <= 0.0 {
        return None;
    }
    let (rect, mut uv) = match fit {
        crate::style::ObjectFit::Contain => {
            let scale = (w / iw).min(h / ih);
            let (dw, dh) = (iw * scale, ih * scale);
            let (dx, dy) = (x + (w - dw) / 2.0, y + (h - dh) / 2.0);
            ([dx, dy, dx + dw, dy + dh], [0.0, 0.0, 1.0, 1.0])
        }
        crate::style::ObjectFit::Cover => {
            let scale = (w / iw).max(h / ih);
            let (shown_w, shown_h) = (w / (iw * scale), h / (ih * scale));
            let (u0, v0) = ((1.0 - shown_w) / 2.0, (1.0 - shown_h) / 2.0);
            ([x, y, x + w, y + h], [u0, v0, u0 + shown_w, v0 + shown_h])
        }
    };
    let [x0, mut y0, x1, mut y1] = rect;
    let (top, bottom) = (y0.max(clip_top), y1.min(clip_bottom));
    if bottom <= top || x1 <= x0 {
        return None;
    }
    let span = y1 - y0;
    let (v0, v1) = (uv[1], uv[3]);
    uv[1] = v0 + (top - y0) / span * (v1 - v0);
    uv[3] = v0 + (bottom - y0) / span * (v1 - v0);
    y0 = top;
    y1 = bottom;
    Some(Placement {
        rect: [x0, y0, x1, y1],
        uv,
    })
}

/// A picture decoded on a thread, ready for the GPU.
pub(crate) struct Loaded {
    pub path: String,
    pub class: u32,
    /// Width, height and RGBA pixels, or `None` if it could not be decoded.
    pub image: Option<(u32, u32, Vec<u8>)>,
}

/// Decodes pictures on a few threads of its own, so a folder of photos never stalls a
/// frame. Ask with [`Loader::request`], collect with [`Loader::poll`].
pub(crate) struct Loader {
    requests: std::sync::mpsc::Sender<(String, u32)>,
    done: std::sync::mpsc::Receiver<Loaded>,
    /// What was asked for and has not come back, so nothing is asked twice.
    asked: std::collections::HashSet<(String, u32)>,
}

impl Loader {
    pub fn new(threads: usize) -> Self {
        let (requests, wanted) = std::sync::mpsc::channel::<(String, u32)>();
        let (finished, done) = std::sync::mpsc::channel::<Loaded>();
        let wanted = std::sync::Arc::new(std::sync::Mutex::new(wanted));
        for _ in 0..threads.max(1) {
            let (wanted, finished) = (std::sync::Arc::clone(&wanted), finished.clone());
            std::thread::spawn(move || {
                loop {
                    // Hold the lock only while waiting for work, so the others can decode.
                    let next = wanted.lock().ok().and_then(|rx| rx.recv().ok());
                    let Some((path, class)) = next else { return };
                    let image = decode_scaled(&path, class).ok();
                    if finished.send(Loaded { path, class, image }).is_err() {
                        return;
                    }
                }
            });
        }
        Loader {
            requests,
            done,
            asked: std::collections::HashSet::new(),
        }
    }

    pub fn request(&mut self, path: &str, class: u32) {
        if self.asked.insert((path.to_owned(), class)) {
            let _ = self.requests.send((path.to_owned(), class));
        }
    }

    /// Up to `max` finished pictures.
    pub fn poll(&mut self, max: usize) -> Vec<Loaded> {
        let mut out = Vec::new();
        while out.len() < max {
            let Ok(loaded) = self.done.try_recv() else {
                break;
            };
            self.asked.remove(&(loaded.path.clone(), loaded.class));
            out.push(loaded);
        }
        out
    }

    /// How many are still being decoded.
    pub fn pending(&self) -> usize {
        self.asked.len()
    }
}

/// Choose the cache slot to evict: prefer empty slots, then pick the LRU.
pub(crate) fn find_evict_slot_in(cache: &[Option<CachedTexture>]) -> usize {
    if let Some(i) = cache.iter().position(|s| s.is_none()) {
        return i;
    }
    cache
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.as_ref().map(|t| (i, t.last_used_frame)))
        .min_by_key(|&(_, f)| f)
        .map(|(i, _)| i)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::ObjectFit;

    /// A PNG of `w` by `h` in a folder of its own. Returns the path.
    fn png(name: &str, w: u32, h: u32) -> String {
        let dir = std::env::temp_dir().join(format!("sicompass-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        img_crate::RgbaImage::from_pixel(w, h, img_crate::Rgba([200, 30, 30, 255]))
            .save(&path)
            .unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_size_is_read_from_the_header() {
        let path = png("size.png", 321, 123);
        assert_eq!(dimensions(&path), Some((321, 123)));
        // And again, from the cache.
        assert_eq!(dimensions(&path), Some((321, 123)));
    }

    #[test]
    fn a_missing_or_broken_picture_has_no_size() {
        assert_eq!(dimensions("/no/such/picture.png"), None);
        let dir = std::env::temp_dir().join(format!("sicompass-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let broken = dir.join("broken.png");
        std::fs::write(&broken, b"not a picture").unwrap();
        assert_eq!(dimensions(&broken.to_string_lossy()), None);
    }

    #[test]
    fn size_classes_grow_with_the_box_and_stop_at_2048() {
        assert_eq!(size_class_for(100.0), 256);
        assert_eq!(size_class_for(170.0), 256);
        assert_eq!(size_class_for(171.0), 512);
        assert_eq!(size_class_for(600.0), 1024);
        assert_eq!(size_class_for(1500.0), 2048);
        assert_eq!(size_class_for(9000.0), 2048);
        assert_eq!(size_class_for(0.0), 256);
    }

    #[test]
    fn a_decoded_picture_is_no_bigger_than_its_class() {
        let path = png("big.png", 1000, 500);
        let (w, h, pixels) = decode_scaled(&path, 256).unwrap();
        assert_eq!((w, h), (256, 128));
        assert_eq!(pixels.len(), (w * h * 4) as usize);
        let (w, h, _) = decode_scaled(&path, 2048).unwrap();
        assert_eq!((w, h), (1000, 500), "never made bigger");
    }

    #[test]
    fn contain_shows_all_of_it_centred() {
        let place = place_picture(
            [0.0, 0.0, 200.0, 100.0],
            (100, 100),
            ObjectFit::Contain,
            0.0,
            1000.0,
        )
        .unwrap();
        assert_eq!(place.rect, [50.0, 0.0, 150.0, 100.0]);
        assert_eq!(place.uv, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn cover_fills_the_box_and_crops_the_middle() {
        // A wide picture in a square box: the sides go.
        let place = place_picture(
            [10.0, 20.0, 100.0, 100.0],
            (400, 100),
            ObjectFit::Cover,
            0.0,
            1000.0,
        )
        .unwrap();
        assert_eq!(place.rect, [10.0, 20.0, 110.0, 120.0]);
        assert_eq!(place.uv[1], 0.0);
        assert_eq!(place.uv[3], 1.0);
        assert!(
            (place.uv[0] - 0.375).abs() < 1e-6 && (place.uv[2] - 0.625).abs() < 1e-6,
            "{place:?}"
        );
    }

    #[test]
    fn a_picture_cut_off_at_the_top_loses_its_top() {
        let place = place_picture(
            [0.0, 100.0, 100.0, 100.0],
            (100, 100),
            ObjectFit::Contain,
            150.0,
            1000.0,
        )
        .unwrap();
        assert_eq!(place.rect, [0.0, 150.0, 100.0, 200.0]);
        assert_eq!(place.uv, [0.0, 0.5, 1.0, 1.0]);
        let place = place_picture(
            [0.0, 100.0, 100.0, 100.0],
            (100, 100),
            ObjectFit::Contain,
            0.0,
            150.0,
        )
        .unwrap();
        assert_eq!(place.uv, [0.0, 0.0, 1.0, 0.5]);
    }

    #[test]
    fn a_picture_clipped_away_or_empty_shows_nothing() {
        assert!(
            place_picture(
                [0.0, 0.0, 100.0, 100.0],
                (10, 10),
                ObjectFit::Contain,
                100.0,
                200.0
            )
            .is_none()
        );
        assert!(
            place_picture(
                [0.0, 0.0, 0.0, 100.0],
                (10, 10),
                ObjectFit::Cover,
                0.0,
                200.0
            )
            .is_none()
        );
        assert!(
            place_picture(
                [0.0, 0.0, 100.0, 100.0],
                (0, 10),
                ObjectFit::Cover,
                0.0,
                200.0
            )
            .is_none()
        );
    }

    #[test]
    fn the_loader_decodes_in_the_background_and_asks_once() {
        let path = png("loader.png", 600, 300);
        let mut loader = Loader::new(2);
        loader.request(&path, 256);
        loader.request(&path, 256);
        assert_eq!(loader.pending(), 1, "the second ask is the same");
        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(loader.poll(4));
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(got.len(), 1);
        let (w, h, pixels) = got[0].image.as_ref().expect("decoded");
        assert_eq!((*w, *h, pixels.len()), (256, 128, 256 * 128 * 4));
        assert_eq!(loader.pending(), 0);
        // Asked for again, it is decoded again.
        loader.request(&path, 256);
        assert_eq!(loader.pending(), 1);
    }

    #[test]
    fn the_loader_reports_a_picture_it_cannot_read() {
        let mut loader = Loader::new(1);
        loader.request("/no/such/picture.png", 256);
        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(loader.poll(4));
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(got.len(), 1);
        assert!(got[0].image.is_none());
    }

    #[test]
    fn a_polled_batch_is_no_bigger_than_asked() {
        let mut loader = Loader::new(2);
        let paths: Vec<String> = (0..6)
            .map(|i| png(&format!("batch{i}.png"), 64, 64))
            .collect();
        for path in &paths {
            loader.request(path, 256);
        }
        let mut total = 0;
        for _ in 0..400 {
            let got = loader.poll(2);
            assert!(got.len() <= 2);
            total += got.len();
            if total == 6 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(total, 6);
    }

    #[test]
    fn find_evict_slot_prefers_empty() {
        let mut cache: Vec<Option<CachedTexture>> = (0..MAX_CACHED_IMAGES).map(|_| None).collect();
        cache[0] = Some(dummy_cached(0, 5));
        cache[1] = Some(dummy_cached(1, 3));
        // Slots 2.. remain None
        let slot = find_evict_slot_in(&cache);
        assert!(slot >= 2, "should prefer an empty slot, got {slot}");
    }

    #[test]
    fn find_evict_slot_lru_when_full() {
        let cache: Vec<Option<CachedTexture>> = (0..MAX_CACHED_IMAGES)
            .map(|i| Some(dummy_cached(i, (i as u64 + 1) * 10)))
            .collect();
        let slot = find_evict_slot_in(&cache);
        // Slot 0 has last_used_frame = 10 — the smallest
        assert_eq!(slot, 0);
    }

    #[test]
    fn find_evict_slot_empty_cache_returns_zero() {
        let cache: Vec<Option<CachedTexture>> = (0..MAX_CACHED_IMAGES).map(|_| None).collect();
        let slot = find_evict_slot_in(&cache);
        assert_eq!(slot, 0);
    }

    // ---- decode_image_source ------------------------------------------------
    //
    // No Vulkan device is needed for these, which is the reason the decode was
    // split out of `load_texture`.

    /// A 1x1 red PNG, encoded here rather than committed as a fixture.
    fn tiny_png() -> Vec<u8> {
        let img = img_crate::DynamicImage::new_rgba8(1, 1);
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, img_crate::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_registered_asset_decodes_from_memory() {
        // Through a resolver rather than `register_bytes`, so this covers the
        // plugin-shaped path too (a closure can own its bytes; `&'static` cannot be
        // conjured in a test without leaking).
        sicompass_sdk::assets::register_resolver(
            "__image_test",
            Box::new(|name| (name == "tiny.png").then(tiny_png)),
        );
        let img = decode_image_source("asset:__image_test/tiny.png").expect("should decode");
        assert_eq!((img.width(), img.height()), (1, 1));
    }

    #[test]
    fn an_unregistered_asset_uri_is_an_error_not_a_panic() {
        // `find_or_load` turns an `Err` into "no draw", the same outcome a missing
        // file has always had.
        let err = decode_image_source("asset:__image_test_missing/nope.png")
            .expect_err("must not resolve");
        assert!(format!("{err:?}").contains("no such asset"), "{err:?}");
    }

    #[test]
    fn a_plain_path_still_goes_to_the_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.png");
        std::fs::write(&path, tiny_png()).unwrap();
        let img = decode_image_source(path.to_str().unwrap()).expect("should decode");
        assert_eq!((img.width(), img.height()), (1, 1));
    }

    #[test]
    fn a_missing_plain_path_is_still_an_error() {
        assert!(decode_image_source("/nonexistent/definitely-not-here.png").is_err());
    }

    fn dummy_cached(idx: usize, last_used: u64) -> CachedTexture {
        CachedTexture {
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            view: vk::ImageView::null(),
            sampler: vk::Sampler::null(),
            path: format!("dummy_{idx}.png"),
            class: 0,
            width: 1,
            height: 1,
            last_used_frame: last_used,
        }
    }
}
