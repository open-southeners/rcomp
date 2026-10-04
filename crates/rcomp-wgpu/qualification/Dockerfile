# Tool image for Linux GPU qualification runs (see ../LINUX_INTEL_HANDOVER.md).
#
# Provides `vulkaninfo` and the reference `lz4` CLI without installing them on
# the host. rcomp itself still runs on the host so it exercises the host's
# Vulkan driver. Match UBUNTU_VERSION to the host release so the container's
# Mesa is the same driver build that rcomp uses.
ARG UBUNTU_VERSION=24.04
FROM ubuntu:${UBUNTU_VERSION}

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        lz4 \
        mesa-vulkan-drivers \
        vulkan-tools \
    && rm -rf /var/lib/apt/lists/*

ENV HOME=/tmp
