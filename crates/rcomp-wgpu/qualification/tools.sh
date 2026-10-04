#!/usr/bin/env sh
# Run a qualification tool (vulkaninfo, lz4, ...) inside the tool image.
#
#   crates/rcomp-wgpu/qualification/tools.sh vulkaninfo --summary
#   crates/rcomp-wgpu/qualification/tools.sh lz4 -t out.lz4
#   crates/rcomp-wgpu/qualification/tools.sh sh -c 'lz4 -dc out.lz4 | cmp - in.bin'
#
# The image is built on first use. The current directory is mounted at the
# same path, so relative and absolute paths under it work unchanged, and the
# command runs as the calling user so output files are not owned by root.
# Set RCOMP_QUAL_REBUILD=1 to rebuild, or RCOMP_QUAL_UBUNTU to change the base
# release (defaults to the host's VERSION_ID).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
image=rcomp-qualification-tools
ubuntu=${RCOMP_QUAL_UBUNTU:-$(. /etc/os-release && echo "${VERSION_ID:-24.04}")}

if [ "$#" -eq 0 ]; then
    echo "usage: $0 COMMAND [ARGS...]" >&2
    exit 2
fi

if [ "${RCOMP_QUAL_REBUILD:-0}" = 1 ] || ! docker image inspect "$image" >/dev/null 2>&1; then
    docker build --quiet --build-arg "UBUNTU_VERSION=$ubuntu" -t "$image" "$here" >&2
fi

set -- --rm -u "$(id -u):$(id -g)" -v "$PWD:$PWD" -w "$PWD" "$image" "$@"

# Pass the GPU render nodes through when present, joining the group that owns
# them so the unprivileged user can open them.
if [ -d /dev/dri ]; then
    for node in /dev/dri/renderD*; do
        [ -e "$node" ] || continue
        set -- --device /dev/dri --group-add "$(stat -c %g "$node")" "$@"
        break
    done
fi

# Allocate a TTY only when attached to one, so pipes and redirects still work.
if [ -t 0 ] && [ -t 1 ]; then
    set -- -it "$@"
else
    set -- -i "$@"
fi

exec docker run "$@"
