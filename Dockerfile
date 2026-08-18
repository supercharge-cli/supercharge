FROM python:3.11-slim

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1

WORKDIR /app

RUN useradd --create-home --uid 10001 hermes

COPY pyproject.toml README.md ./
COPY agent ./agent

RUN pip install --no-cache-dir ".[e2b]"

RUN mkdir -p /app/runtime && chown -R hermes:hermes /app
USER hermes

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD python -c "import urllib.request; urllib.request.urlopen('http://127.0.0.1:8080/healthz', timeout=3)"

CMD ["uvicorn", "agent.main:app", "--host", "0.0.0.0", "--port", "8080"]
