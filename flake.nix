{
  description = "sicompass-ui, the Vulkan/SDL3 list renderer shared by sicompass and its login greeter";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # Intel macOS only. nixpkgs 26.11 dropped x86_64-darwin outright, and it
    # does not merely stop building: `import nixpkgs { system =
    # "x86_64-darwin"; }` throws, so a single-input flake that lists the system
    # fails to evaluate on *every* platform, not just that one. 26.05 is the
    # last branch carrying it, and it gets security fixes until the end of
    # 2026. Retire this input, and the system below, when that runs out.
    nixpkgs-x86-darwin.url = "github:NixOS/nixpkgs/nixpkgs-26.05-darwin";

    # Splits each package into a dependency build keyed on Cargo.lock alone and
    # the workspace crates on top, so an edit to sicompass itself does not
    # recompile the whole dependency graph. See `buildMember` below.
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, nixpkgs-x86-darwin, crane }:
    let
      supportedSystems = [
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-linux"
        "x86_64-darwin"
      ];

      # Which nixpkgs a given system is built from. Everything tracks unstable
      # except Intel macOS, which unstable no longer has, per the input note.
      nixpkgsInputFor = system:
        if system == "x86_64-darwin" then nixpkgs-x86-darwin else nixpkgs;

      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      nixpkgsFor = forAllSystems (system:
        import (nixpkgsInputFor system) { inherit system; });

      # Single source of truth for the version.
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
    in
    {
      devShells = forAllSystems (system:
        let
          pkgs = nixpkgsFor.${system};
        in
        {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              # Rust toolchain
              cargo
              rustc
              rust-analyzer
              clippy
              rustfmt



              # Native libs required by Rust crates
              pkg-config
              sdl3
              freetype
              libwebp

              # cmake: several -sys crates drive a CMake build. sdl3-sys needs
              # it for the `bundled-sdl3` feature (which compiles the vendored
              # SDL 3.4.12 from source), and aws-lc-sys and libsqlite3-sys need
              # it unconditionally. Without it `cargo build --features
              # bundled-sdl3` dies in sdl3-sys' build script with
              # "is `cmake` not installed?".
              cmake

              # Vulkan (used via ash crate)
              spirv-tools
              vulkan-loader
              vulkan-headers
              glslang


            ] ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
              # xvfb-run: the renderer tests that open an SDL window run under
              # an invisible X11 display on a headless machine or in CI.
              xvfb-run
              vulkan-volk
              vulkan-tools
              vulkan-validation-layers
              vulkan-extension-layer
              vulkan-tools-lunarg
              wayland
              wayland-scanner
              wayland-protocols
              libxkbcommon

              # SDL3's own build dependencies, needed only by the
              # `bundled-sdl3` feature, which compiles the vendored SDL from
              # source. SDL's CMake aborts configure with "could not find X11
              # or Wayland development libraries" unless it can see at least
              # one windowing backend, and it probes for the audio and DRM
              # backends the same way. The release build uses this feature, so
              # the dev shell has to be able to reproduce it. Mirrors the apt
              # list in `[dist.dependencies.apt]`.
              libx11
              libxext
              libxcursor
              libxi
              libxrandr
              libxscrnsaver
              libxfixes
              libxrender
              libxtst
              # xcb: SDL's bundled vulkan.h includes <xcb/xcb.h> for the
              # VK_USE_PLATFORM_XCB_KHR surface path.
              libxcb
              libdecor
              libGL
              libdrm
              mesa
              alsa-lib
              libpulseaudio

              # Accessibility (accesskit_unix)
              at-spi2-core
              dbus
              accerciser

              # udev for SDL's device enumeration.
              udev
              # gbm is its own package in this nixpkgs (mesa-libgbm); it is no
              # longer part of the mesa output, so `gbm.pc` is only found with
              # this listed explicitly. The bundled SDL build probes for it.
              libgbm
            ] ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
              # MoltenVK is the only Vulkan driver on macOS: it implements
              # Vulkan on top of Metal, and vulkan-loader enumerates zero ICDs
              # without it. Nothing on the Linux list has a macOS counterpart
              # to add here, because SDL and accesskit both go through Cocoa:
              # no Wayland, no X11, no xkb, no D-Bus, no Mesa.
              moltenvk
            ];

            # The hook is split three ways because macOS shares almost none of
            # the Linux graphics stack. `wayland` does not merely fail to
            # build on darwin, it fails to *evaluate* (nixpkgs marks
            # aarch64-darwin in its meta.badPlatforms), so an unconditional
            # reference to it here took down `nix develop` on macOS at eval
            # time, before any package was fetched.
            shellHook = with pkgs; ''
              # Rust stdlib source for rust-analyzer
              export RUST_SRC_PATH="${pkgs.rustc}/lib/rustlib/src/rust/library";

              # SDL3 pkg-config / link path (needed by sdl3-rs / cargo build)
              export PKG_CONFIG_PATH="${sdl3}/lib/pkgconfig:$PKG_CONFIG_PATH";
              export VULKAN_SDK="${vulkan-headers}";
            ''
            + lib.optionalString stdenv.hostPlatform.isLinux ''
              export PKG_CONFIG_PATH="${libxkbcommon.dev}/lib/pkgconfig:$PKG_CONFIG_PATH";
              export LIBRARY_PATH="${sdl3}/lib:${libxkbcommon}/lib:${wayland}/lib:${libGL}/lib:${mesa}/lib:${udev}/lib:${libgbm}/lib:$LIBRARY_PATH";

              # Library path for Vulkan and other runtime deps.
              #
              # Store paths only: LD_LIBRARY_PATH outranks the DT_RUNPATH Nix
              # bakes into its binaries, so a system lib dir here is resolved
              # first by *every* binary in the shell. On a distro whose glibc is
              # older than nixpkgs' (Mint 22.3 ships 2.39, nixpkgs-unstable is on
              # 2.42), adding /usr/lib/x86_64-linux-gnu bricks the shell: sh, rm
              # and uname all die with "version `GLIBC_2.42' not found".
              #
              # The system Mesa ICDs cannot make up for it either, see the
              # VK_ICD_FILENAMES block below.
              # libGL (libglvnd) and libgbm are dispatch libraries, which is
              # exactly why they are safe to take from the shell: they load a
              # vendor at runtime, and the vendor has to be the system's, see
              # the block below. Note what is *not* here: nixpkgs' `mesa`.
              #
              # This variable is an assignment with no ":$LD_LIBRARY_PATH"
              # tail, so anything missing from it is excluded outright rather
              # than falling back to the system.
              export LD_LIBRARY_PATH="${libwebp}/lib:${freetype}/lib:${vulkan-loader}/lib:${vulkan-validation-layers}/lib:${sdl3}/lib:${libxkbcommon}/lib:${wayland}/lib:${libGL}/lib:${udev}/lib:${libgbm}/lib";
              export VK_LAYER_PATH="${vulkan-validation-layers}/share/vulkan/explicit_layer.d";

              # The GL/EGL/GBM *vendor*, as opposed to the dispatch libraries
              # above.
              #
              # On NixOS this must be /run/opengl-driver, the driver the rest
              # of the running system uses, and never nixpkgs' own `mesa`.
              # Taking the vendor from the shell instead puts two Mesa builds
              # in one process: GBM loads its backend and DRI driver from the
              # system while libEGL resolves to the shell's, and the first
              # call across that boundary segfaults. Observed exactly that on
              # the TTY backend - eglQueryDmaBufModifiersEXT entered
              # dri_query_dma_buf_modifiers in libgallium-26.2.3 and landed in
              # si_memobj_destroy in libgallium-26.1.8.
              #
              # These three cover the three lookups Mesa does: which EGL
              # vendor glvnd loads, where the DRI driver comes from, and which
              # GBM backend libgbm dlopens.
              if [ -d /run/opengl-driver/lib ]; then
                export __EGL_VENDOR_LIBRARY_DIRS="/run/opengl-driver/share/glvnd/egl_vendor.d";
                export LIBGL_DRIVERS_PATH="/run/opengl-driver/lib/dri";
                export GBM_BACKENDS_PATH="/run/opengl-driver/lib/gbm";
              else
                # Not NixOS: no system driver tree, so the shell's own Mesa is
                # the only one in play and mixing cannot happen.
                export __EGL_VENDOR_LIBRARY_DIRS="${mesa}/share/glvnd/egl_vendor.d";
              fi

              # Vulkan ICD discovery on non-NixOS distros.
              #
              # /usr/share/vulkan/icd.d is no use to us. Every manifest there
              # names its driver relatively ("libvulkan_intel.so",
              # "libGLX_nvidia.so.0"), and nixpkgs' glibc ships no
              # ld.so.cache, so the loader's dlopen has nothing but
              # LD_LIBRARY_PATH to search and resolves none of them. The app
              # then dies with SDL's "Vulkan doesn't implement the
              # VK_KHR_surface extension", which is what zero usable ICDs
              # looks like from the outside. Symlinking the drivers onto the
              # path does not rescue it: they fail on their own dependencies
              # (libdrm.so.2, libLLVM.so.20.1) for the same reason. Adding
              # /usr/lib/x86_64-linux-gnu would resolve every one of them and
              # brick the shell, per the note above.
              #
              # nixpkgs' Mesa carries absolute store paths in its manifests,
              # so Intel, AMD (radv) and the llvmpipe software fallback all
              # load with no system library involved. Prefer it.
              #
              # Mesa also ships nouveau, but that does not amount to NVIDIA
              # support: nouveau is the open reimplementation, and on a
              # machine running the proprietary driver it is blacklisted and
              # never binds the card. NVIDIA is handled separately below.
              #
              # On NixOS the drivers live in /run/opengl-driver and the loader
              # finds them unaided, so leave VK_ICD_FILENAMES unset there:
              # pointing it at a missing path makes the loader report zero
              # ICDs and produces exactly the same SDL failure.
              if [ ! -d /run/opengl-driver ]; then
                _icd=$(ls ${mesa}/share/vulkan/icd.d/*.json 2>/dev/null | tr '\n' ':' | sed 's/:$//');

                # Proprietary NVIDIA is the one driver nixpkgs cannot stand in
                # for: it has to match the running kernel module, so it can
                # only come from the host. Unlike Mesa it has no dependency
                # fan-out beyond libc, so a symlink farm of just libGLX_nvidia
                # and libnvidia-* is enough for dlopen to resolve it without
                # putting the system glibc on the path. Listed ahead of Mesa
                # so it wins on a machine that has both.
                if [ -e /usr/share/vulkan/icd.d/nvidia_icd.json ]; then
                  _farm="$HOME/.cache/sicompass/vk-nvidia";
                  mkdir -p "$_farm";
                  for _lib in /usr/lib/x86_64-linux-gnu/libGLX_nvidia.so.* \
                              /usr/lib/x86_64-linux-gnu/libnvidia-*.so.*; do
                    [ -e "$_lib" ] && ln -sfn "$_lib" "$_farm/$(basename "$_lib")";
                  done
                  export LD_LIBRARY_PATH="$_farm:$LD_LIBRARY_PATH";
                  _icd="/usr/share/vulkan/icd.d/nvidia_icd.json:$_icd";
                  unset _farm _lib;
                fi

                [ -n "$_icd" ] && export VK_ICD_FILENAMES="$_icd";
                unset _icd;
              fi
            ''
            + lib.optionalString stdenv.hostPlatform.isDarwin ''
              export LIBRARY_PATH="${sdl3}/lib:$LIBRARY_PATH";

              # dyld, not ld.so. Nix's darwin linker bakes an absolute store
              # path into each dylib's install name, so anything cargo *links*
              # resolves with no search path at all. This is here for what the
              # app dlopens instead: `ash::Entry::load()` asks for
              # libvulkan.1.dylib by bare name, and MoltenVK is loaded in turn
              # by the loader, so neither is reachable without it.
              #
              # FALLBACK rather than DYLD_LIBRARY_PATH: the fallback list is
              # consulted last, so it cannot shadow a system framework the way
              # the Linux LD_LIBRARY_PATH note warns about.
              #
              # This variable cannot be relied on, and SDL_VULKAN_LIBRARY below
              # is what actually carries the day. System Integrity Protection
              # strips every DYLD_* variable across an exec of a protected
              # binary, and /bin is protected, so the `exec "$_sh"` at the end
              # of this hook destroys it for anyone whose login shell is
              # /bin/zsh, which is the macOS default. It survives only for
              # `nix develop -c <cmd>`, which does not exec a login shell.
              # Kept for exactly that case, and for libraries other than the
              # Vulkan pair.
              export DYLD_FALLBACK_LIBRARY_PATH="${vulkan-loader}/lib:${moltenvk}/lib:${sdl3}/lib:${freetype}/lib:${libwebp}/lib:$HOME/lib:/usr/local/lib:/usr/lib";

              # The SIP-proof half of the above: an ordinary variable name, so
              # it survives the exec into the user's shell. SDL3 reads it for
              # its own loader (SDL_HINT_VULKAN_LIBRARY) and sicompass reads
              # the same name in `load_vulkan_entry`, so one value steers both.
              # Point it at MoltenVK directly rather than the loader, since
              # MoltenVK exports the Vulkan entry points itself.
              export SDL_VULKAN_LIBRARY="${moltenvk}/lib/libMoltenVK.dylib";

              # One ICD, always present, and its manifest carries an absolute
              # store path. So unlike the Linux branch there is nothing to
              # probe for and no symlink farm to build: point at it and stop.
              export VK_ICD_FILENAMES="${moltenvk}/share/vulkan/icd.d/MoltenVK_icd.json";
            ''
            + ''
              # Do NOT add -fuse-ld=lld for the host target here. It halves
              # linker memory, which is tempting on a small machine, but gcc
              # then invokes ld.lld directly and bypasses Nix's ld wrapper,
              # which is what injects the store paths into RUNPATH. The
              # binaries still link, so a plain `cargo build` looks fine, and
              # then every test binary dies at startup with
              #   error while loading shared libraries: libssl.so.3
              # because its RUNPATH is only the placeholder outputs/out/lib.

              # Cap parallel cargo jobs by RAM as well as cores. A single rustc
              # can hold ~1 GB, so -j nproc overcommits badly on a small
              # machine while a browser and an editor are also resident. One job per 2 GB, never above nproc.
              # A no-op on any machine with enough RAM to cover its cores.
              #
              # The probe is the platform-specific part: macOS has neither
              # /proc/meminfo nor nproc, so it answers the same two questions
              # through sysctl. The cap itself is shared.
              if [ -z "$CARGO_BUILD_JOBS" ]; then
                if [ -r /proc/meminfo ]; then
                  _gb=$(awk '/MemTotal/{printf "%d", $2/1048576}' /proc/meminfo);
                  _cores=$(nproc);
                elif command -v sysctl >/dev/null 2>&1; then
                  _gb=$(( $(sysctl -n hw.memsize) / 1073741824 ));
                  _cores=$(sysctl -n hw.ncpu);
                fi
                if [ -n "$_gb" ]; then
                  _cap=$((_gb / 2));
                  [ "$_cap" -lt 1 ] && _cap=1;
                  if [ "$_cap" -lt "$_cores" ]; then
                    export CARGO_BUILD_JOBS="$_cap";
                  fi
                  unset _cap;
                fi
                unset _gb _cores;
              fi

              # Hand interactive sessions to whatever shell the user actually
              # uses. `nix develop` always starts bash, which is correct for
              # `nix develop -c <cmd>` but not what a person wants to be typing
              # into.
              #
              # The user's login shell rather than a hardcoded name: this used
              # to be a bare `exec fish`, which drops any machine without fish
              # installed straight into "fish: command not found" the moment
              # `nix develop` finishes, with no shell left to type into. fish
              # is not in buildInputs and deliberately still is not, because
              # the point is to honour the user's choice, not to ship a second
              # one. Set SICOMPASS_DEV_SHELL to override; set it to `bash` to
              # stay in the bash that nix develop provides.
              #
              # Do NOT reach for $SHELL here, however obvious it looks: `nix
              # develop` overwrites it with its own store bash before this hook
              # runs, so it reports bash on every machine and this block
              # silently never fires. The login shell has to come from the OS
              # user database, and that is the platform-specific part, so it is
              # asked three ways and the first hit wins.
              #
              # The [ -t 0 ] guard is load-bearing and must stay. `nix develop
              # -c <cmd>` and tooling (Claude Code's Bash tool, CI) get no tty;
              # exec'ing a shell there replaces the process and silently
              # discards the command, which exits 0 with no output.
              #
              # Note the absence of a login flag. `exec "$_sh"` starts a
              # non-login shell on purpose: on macOS a login shell sources
              # /etc/zprofile, which runs path_helper, which reorders PATH to
              # put /usr/bin ahead of everything and would bury this shell's
              # toolchain behind the system one.
              if [ -t 0 ]; then
                _sh="$SICOMPASS_DEV_SHELL";
                _me=$(id -un);

                # getent is glibc's nsswitch front end, so it is the only one
                # of the three that also answers for LDAP/SSSD accounts with
                # no local passwd line. Linux-only, and not reliably on PATH
                # inside a nix shell (it lives in glibc's `bin` output, which
                # is not a build input here), hence the plain-file read next.
                if [ -z "$_sh" ] && command -v getent >/dev/null 2>&1; then
                  _sh=$(getent passwd "$_me" 2>/dev/null | cut -d: -f7);
                fi

                # /etc/passwd needs no binary at all, which is what makes it
                # the dependable path on Linux, NixOS included: NixOS
                # generates a real passwd file for local users. Harmlessly
                # empty on macOS, where this file lists only system accounts.
                if [ -z "$_sh" ] && [ -r /etc/passwd ]; then
                  _sh=$(awk -F: -v u="$_me" '$1 == u { print $7 }' /etc/passwd);
                fi

                # macOS keeps local accounts in Directory Service instead.
                if [ -z "$_sh" ] && command -v dscl >/dev/null 2>&1; then
                  _sh=$(dscl . -read "/Users/$_me" UserShell 2>/dev/null \
                        | sed 's/^UserShell: *//');
                fi
                unset _me;
                case "''${_sh##*/}" in
                  # Already in bash, and nix's bash is set up for this shell.
                  bash | "") ;;
                  *) command -v "$_sh" >/dev/null 2>&1 && exec "$_sh" ;;
                esac
                unset _sh;
              fi
            '';
          };
        });


      # Proves the crate builds under Nix, with the same source filter and
      # toolchain a dependant's Nix build sees. There is no binary to install:
      # this is a library, consumed by git tag from sicompass and loginsicompass.
      packages = forAllSystems (system:
        let
          pkgs = nixpkgsFor.${system};
          craneLib = crane.mkLib pkgs;
          lib = pkgs.lib;
          commonArgs = {
            inherit version;
            pname = "sicompass-ui";
            # crane's default filter keeps Cargo and .rs files only, and this
            # crate include_bytes!/include_str!s shaders, fonts, the window
            # icon and its Fluent bundles.
            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                (craneLib.fileset.commonCargoSources ./.)
                ./shaders
                ./fonts
                ./assets
                ./locales
              ];
            };
            strictDeps = true;
            # The renderer tests want a display; ci.yml runs them under xvfb.
            doCheck = false;
            nativeBuildInputs = with pkgs; [ pkg-config rustPlatform.bindgenHook ];
            buildInputs = with pkgs; [ sdl3 freetype libwebp ]
              ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [
                wayland
                libxkbcommon
                at-spi2-core
                dbus
              ];
          };
        in
        {
          default = craneLib.cargoBuild (commonArgs // {
            cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          });
        });
    };
}
