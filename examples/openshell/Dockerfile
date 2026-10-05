# The image Factory's `sandbox: openshell` runs a claude-code agent in (#218),
# for a gateway on the Docker or Podman driver. build-image.sh stages the
# build context and runs this; on a Mac without Docker it builds the same
# filesystem as a rootfs tar for the MicroVM driver instead.
#
# On top of NVIDIA's community base (claude, git, gh, curl, node, python):
#   /usr/local/bin/factory   Factory's CLI, a static Linux build -- reports go
#                            to the daemon over FACTORY_URL, not the socket
#   /usr/local/bin/herdr     herdr's Linux build, for `herdr --version` checks
#   /usr/local/bin/jq        jq
#   /etc/claude-code/managed-settings.json   the allow rules an unattended run
#                            needs (git push, gh pr create/merge, ...), so it
#                            never waits on a permission prompt -- and no
#                            dynamic workflows, questions or plan mode, which
#                            each stop for an answer nobody is there to give
#   /etc/gitconfig           gh as git's GitHub credential helper, and the
#                            commit identity
#   /sandbox/.claude.json    onboarding done, /sandbox/work trusted
#   /sandbox/.config/gh/config.yml   gh's own telemetry off (#274), so it's
#                            off for every invocation of gh, including as
#                            git's credential helper, not only ones that
#                            inherit Claude Code's own `env` block
#
# The base image's own /etc/openshell/policy.yaml is removed: OpenShell 0.1.2
# rejects it, and Factory always passes the scope's policy with --policy.

ARG BASE_IMAGE=ghcr.io/nvidia/openshell-community/sandboxes/base@sha256:aeef1c63f00e2913ea002ccb3aaf925f338b5c5d70e63576f0d95c16a138044e
FROM ${BASE_IMAGE}
USER root
RUN rm -f /etc/openshell/policy.yaml
COPY --chmod=0755 usr/local/bin/ /usr/local/bin/
COPY etc/ /etc/
COPY --chown=sandbox:sandbox sandbox/.claude.json /sandbox/.claude.json
COPY --chown=sandbox:sandbox sandbox/.config/ /sandbox/.config/
USER sandbox
