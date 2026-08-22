# syntax=docker/dockerfile:1
# Supercharge interactive agent template for Vectra Cloud / E2B.
# Build context: sandbox/e2b-agent-template (binaries staged by prepare-e2b-agent-assets.sh).
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
ENV PLAYWRIGHT_BROWSERS_PATH=/home/user/.cache/ms-playwright
ENV AGENT_BROWSER_STREAM_PORT=9223
ENV AGENT_BROWSER_STREAM_QUALITY=40
ENV AGENT_BROWSER_STREAM_MAX_WIDTH=960
ENV AGENT_BROWSER_STREAM_MAX_HEIGHT=540

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates openssl curl git git-lfs jq openssh-client \
    python3 python3-venv build-essential xz-utils tar ripgrep fd-find fzf tree \
    tmux lsof psmisc unzip zip wget rsync procps less \
    fonts-liberation libasound2t64 libatk-bridge2.0-0t64 libatk1.0-0t64 \
    libcups2t64 libdbus-1-3 libdrm2 libgbm1 libgtk-3-0t64 libnspr4 libnss3 \
    libx11-xcb1 libxcomposite1 libxdamage1 libxrandr2 xdg-utils \
    && rm -rf /var/lib/apt/lists/*
RUN ln -sf /usr/bin/fdfind /usr/local/bin/fd

RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
      > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update && apt-get install -y --no-install-recommends gh \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get update && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*
RUN corepack enable && corepack prepare pnpm@latest --activate
RUN curl -fsSL https://bun.sh/install | BUN_INSTALL=/usr/local bash
RUN curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh

RUN npm install -g agent-browser \
    && (agent-browser install --with-deps || agent-browser install)

RUN set -e; \
    if ! id -u user >/dev/null 2>&1; then \
      if getent passwd 1000 >/dev/null 2>&1; then \
        existing="$(getent passwd 1000 | cut -d: -f1)"; \
        [ "$existing" = user ] || usermod -l user -d /home/user -m "$existing" || true; \
      else \
        useradd -m -u 1000 -s /bin/bash user; \
      fi; \
    fi; \
    mkdir -p /home/user/workspace /home/user/.cache /home/user/.agent-browser /var/lib/superagent; \
    cp -a /root/.cache/. /home/user/.cache/ 2>/dev/null || true; \
    cp -a /root/.agent-browser/. /home/user/.agent-browser/ 2>/dev/null || true; \
    chown -R 1000:1000 /home/user /var/lib/superagent

COPY bin/supercharge-superagent-server /usr/local/bin/supercharge-superagent-server
COPY bin/supercharge /usr/local/bin/supercharge
RUN ln -sf /usr/local/bin/supercharge /usr/local/bin/sc
COPY wake-http /opt/vectra/wake-http
COPY entrypoint.sh /opt/vectra/entrypoint.sh
RUN chmod +x /usr/local/bin/supercharge-superagent-server /usr/local/bin/supercharge \
    /opt/vectra/entrypoint.sh /opt/vectra/wake-http/start.sh \
    /opt/vectra/wake-http/server.py \
    && chown -R 1000:1000 /opt/vectra

USER user
WORKDIR /home/user
ENV PATH="/usr/local/bin:${PATH}"
ENV SUPERAGENT_BIND=0.0.0.0:8790
ENV SUPERAGENT_DAEMON_WORKDIR=/home/user/workspace
ENV SUPERAGENT_SHARE_WORKSPACE_ROOT=/home/user/workspace
ENV SUPERAGENT_MEDIA_ROOT=/home/user/workspace/.superagent-media
ENV SUPERAGENT_MEMORY_STATE=/var/lib/superagent/memory.json
ENV SUPERAGENT_JOB_STATE=/var/lib/superagent/jobs.json
ENV SUPERAGENT_SCHEDULE_STATE=/var/lib/superagent/schedules.json
ENV SUPERAGENT_CONTROL_STATE=/var/lib/superagent/controls.json
ENV SUPERAGENT_GATEWAY_STATE=/var/lib/superagent/gateway-state.json
ENV SUPERAGENT_E2B_SESSION_STATE=/var/lib/superagent/e2b-sessions.json
ENV SUPERAGENT_E2B_WORKSPACE_ROOT=/home/user/workspace
ENV SUPERAGENT_AGENT_BINARY=/usr/local/bin/supercharge
ENV SUPERAGENT_DAEMON_COMMAND=/usr/local/bin/supercharge-superagent-server
ENV SUPERAGENT_PROVIDER=pollinations
ENV SUPERAGENT_MODEL=kimi-code
ENV SUPERAGENT_PROVIDER_BASE_URL=https://gen.pollinations.ai/v1
ENV SUPERAGENT_PROVIDER_FORMAT=openai
ENV SUPERAGENT_PROVIDER_API_KEY_ENV=POLLINATIONS_API_KEY
ENV SUPERAGENT_PROVIDER_CONFIG_PATH=/var/lib/superagent/provider-config.toml
ENV SUPERAGENT_WAKE_PORT=8788
ENV SUPERAGENT_BROWSER_CONTROL_ENABLED=true

EXPOSE 8788 8790 9223
ENTRYPOINT ["/opt/vectra/entrypoint.sh"]
