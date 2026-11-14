#!/usr/bin/env bash
# Dependency contracts for reduced engine builds.
#
#   scripts/feature-deps.sh            check every contract below
#   scripts/feature-deps.sh self-test  prove a forbidden dependency fails the check
#   VERBOSE=1 scripts/feature-deps.sh  also print each build's crate list
#
# Each contract resolves one package alone (`cargo tree -p <package>`),
# never the workspace: other members turn engine features on (the C library
# asks for `3d` and `navmesh`, the MCP server's tests for the defaults), and
# Cargo unifies features across every package it builds together. Only normal
# dependencies count, for every target platform, so a contract holds for
# desktop and web builds alike.
#
# A contract lists crates that must NOT be in that build. Adding a crate to a
# reduced build is fine; pulling a whole optional subsystem in is the bug this
# catches. When a contract fails, find the edge with
#   cargo tree -p <package> <features> -e normal --target all -i <crate>
set -euo pipefail

# Subsystems, by the crates that bring them in.
RENDER="wgpu wgpu-core wgpu-hal naga winit pollster"
AUDIO="kira cpal"
INSPECTOR="egui egui-wgpu egui-winit"
GAMEPAD="gilrs"
MODELS="gltf"
PHYSICS="rapier2d rapier3d"
NAVMESH="navmesh-rs"
TOOLS="puffin puffin_http subsecond dioxus-devtools"
STEAM="steamworks"
# Image decoders beyond PNG. Each is an opt-in `image-*` feature; JPEG is a
# default one (`image-jpeg`), so builds without defaults must not carry it.
JPEG="zune-jpeg"
OTHER_IMAGES="image-webp gif tiff qoi exr"
# The screen-reader adapter is opt-in (`accessibility`): AccessKit's macOS,
# Windows (UI Automation) and Linux (AT-SPI over zbus) adapters. The
# `accesskit` data types alone also come with the inspector's egui, so builds
# that may have the inspector forbid only the adapters (ACCESS); builds
# without a window or an inspector forbid the data types too (ACCESS_ALL).
# Compressed textures are opt-in (`compressed-textures`, `basis-universal`).
TEXTURES="ktx2 ruzstd texture2ddecoder basis-universal"
ACCESS="accesskit_macos accesskit_windows accesskit_unix accesskit_consumer accesskit_atspi_common atspi zbus"
ACCESS_ALL="$ACCESS accesskit"
# Text shaping is opt-in (`text-shaping`); the inspector's egui brings the
# shaper itself (harfrust) but not the bidirectional, line-break and script
# tables, so builds that may have the inspector forbid only those (SHAPING);
# builds without it forbid the shaper too (SHAPING_ALL). System fonts
# (`system-fonts`) are opt-in on top.
SHAPING="unicode-bidi unicode-linebreak unicode-script fontdb"
SHAPING_ALL="$SHAPING harfrust read-fonts"

# name | cargo package and feature arguments | forbidden crates
CONTRACTS=(
  "headless|-p ponko --no-default-features|$RENDER $AUDIO $INSPECTOR $GAMEPAD $MODELS $PHYSICS $NAVMESH $TOOLS $STEAM $ACCESS_ALL $JPEG $OTHER_IMAGES $SHAPING_ALL $TEXTURES"
  "headless with audio|-p ponko --no-default-features --features audio|$RENDER $INSPECTOR $GAMEPAD $MODELS $PHYSICS $NAVMESH $TOOLS $STEAM $ACCESS_ALL $JPEG $OTHER_IMAGES $SHAPING_ALL $TEXTURES"
  "window-only 2D|-p ponko --no-default-features --features window|$AUDIO $INSPECTOR $GAMEPAD $MODELS $PHYSICS $NAVMESH $TOOLS $STEAM $ACCESS $JPEG $OTHER_IMAGES $SHAPING_ALL $TEXTURES"
  # A 2D game with JPEG backgrounds and WebP tiles: image decoders, no 3D.
  "2D with image formats|-p ponko --no-default-features --features window,image-jpeg,image-webp|$AUDIO $INSPECTOR $GAMEPAD $MODELS $PHYSICS $NAVMESH $TOOLS $STEAM $ACCESS $SHAPING_ALL $TEXTURES"
  "default|-p ponko|$MODELS $PHYSICS $NAVMESH $TOOLS $STEAM $ACCESS $OTHER_IMAGES $SHAPING $TEXTURES"
  # A localized 2D game: the shaper and its Unicode tables, nothing else.
  # The adapters bring only themselves and their system bindings (zbus on
  # Linux), no other subsystem.
  "2D with accessibility|-p ponko --no-default-features --features accessibility|$AUDIO $INSPECTOR $GAMEPAD $MODELS $PHYSICS $NAVMESH $TOOLS $STEAM $JPEG $OTHER_IMAGES $SHAPING_ALL $TEXTURES"
  "2D with text shaping|-p ponko --no-default-features --features window,text-shaping|$AUDIO $INSPECTOR $GAMEPAD $MODELS $PHYSICS $NAVMESH $TOOLS $STEAM $ACCESS $JPEG $OTHER_IMAGES fontdb $TEXTURES"
  # The C library always turns on the engine's `3d` and `navmesh` features:
  # its 3D and navigation functions are exported unconditionally (the C
  # header declares them all). So its smallest build carries glTF and Recast,
  # but nothing else optional. Making them optional changes the native
  # interface and is separate work.
  "C library, no defaults|-p ponko_ffi --no-default-features|$RENDER $AUDIO $INSPECTOR $GAMEPAD $PHYSICS $TOOLS $STEAM $ACCESS_ALL $TEXTURES"
)

# The crate names in one build, sorted and unique.
closure() {
  # shellcheck disable=SC2086 # package and feature arguments are meant to split
  cargo tree --locked -q $1 -e normal --target all --prefix none --format '{p}' |
    awk 'NF { print $1 }' | sort -u
}

# check NAME FEATURES FORBIDDEN: prints a line per problem, returns 1 on any.
check() {
  local name=$1 features=$2 forbidden=$3 crates bad=()
  crates=$(closure "$features")
  for crate in $forbidden; do
    if grep -qxF "$crate" <<<"$crates"; then bad+=("$crate"); fi
  done
  local count
  count=$(wc -l <<<"$crates" | tr -d ' ')
  if [ "${VERBOSE:-}" = 1 ]; then printf '%s\n' "$crates" | sed 's/^/    /'; fi
  if [ ${#bad[@]} -eq 0 ]; then
    printf 'ok    %-22s %4s crates  (cargo tree %s)\n' "$name" "$count" "$features"
  else
    printf 'FAIL  %-22s %4s crates  (cargo tree %s)\n' "$name" "$count" "$features"
    printf '      must not depend on: %s\n' "${bad[*]}"
    return 1
  fi
}

run_all() {
  local failed=0
  for contract in "${CONTRACTS[@]}"; do
    IFS='|' read -r name features forbidden <<<"$contract"
    check "$name" "$features" "$forbidden" || failed=1
  done
  return $failed
}

case "${1:-check}" in
  check)
    if run_all; then
      echo "Every reduced build stays within its dependency contract."
    else
      echo "A build pulls in a subsystem it must not; see above." >&2
      exit 1
    fi
    ;;
  self-test)
    # The headless contract must reject a build that turns on 3D (glTF) and
    # 2D physics (rapier2d) by accident.
    if check "headless + 3d,physics" "-p ponko --no-default-features --features 3d,physics" \
      "${CONTRACTS[0]##*|}" >/dev/null; then
      echo "self-test failed: the headless contract accepted glTF and rapier2d" >&2
      exit 1
    fi
    # And one that turns the JPEG decoder on must fail it too.
    if check "headless + image-jpeg" "-p ponko --no-default-features --features image-jpeg" \
      "${CONTRACTS[0]##*|}" >/dev/null; then
      echo "self-test failed: the headless contract accepted the JPEG decoder" >&2
      exit 1
    fi
    echo "self-test ok: an accidentally enabled subsystem fails the headless contract."
    ;;
  *)
    echo "usage: $0 [check|self-test]" >&2
    exit 2
    ;;
esac
