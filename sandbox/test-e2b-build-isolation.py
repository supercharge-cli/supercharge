#!/usr/bin/env python3
"""Check E2B build paths without building, modifying assets, or contacting E2B."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile

source = Path(__file__).with_name("prepare-e2b-agent-assets.sh")
with tempfile.TemporaryDirectory(prefix="supercharge-build-isolation-") as directory:
    root = Path(directory)
    (root / "sandbox/browser-runtime").mkdir(parents=True)
    (root / "sandbox/e2b-builder").mkdir()
    (root / ".cache/e2b-vendor").mkdir(parents=True)
    (root / "tools").mkdir()
    for name in ["package.json", "server.mjs", "lease.mjs", "handoff.mjs", "desktop.mjs", "capture.py", "release-input.py", "launch-browser.mjs", "start.sh"]:
        (root / "sandbox/browser-runtime" / name).write_text("fixture\n")
    (root / ".cache/e2b-vendor/rg-15.0.0-x86_64-unknown-linux-musl").write_text("fixture\n")
    script = root / "sandbox/prepare-e2b-agent-assets.sh"
    shutil.copyfile(source, script)
    docker = root / "tools/docker"
    docker.write_text("""#!/usr/bin/env python3
import json, os, sys
with open(os.environ['DOCKER_ARGS'], 'a') as output:
    output.write(json.dumps(sys.argv[1:]) + '\\n')
""")
    docker.chmod(0o755)
    args_file = root / "docker-args.jsonl"
    result = subprocess.run(
        ["bash", str(script)],
        env={
            **os.environ,
            "PATH": str(root / "tools") + os.pathsep + os.environ["PATH"],
            "DOCKER_ARGS": str(args_file),
            "SUPERCHARGE_E2B_BUILD_LOCAL": "0",
            "SUPERCHARGE_E2B_DAEMON_ONLY": "0",
        },
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    import json
    calls = [json.loads(line) for line in args_file.read_text().splitlines()]
    run = next(call for call in calls if call[0] == "run")
    assert "CARGO_TARGET_DIR=/work/target-e2b-release" in run
    command = run[-1]
    assert '$CARGO_TARGET_DIR/release/supercharge-superagent-server' in command
    assert '$CARGO_TARGET_DIR/release/supercharge-pager' in command
    assert "install -Dm755 target/release/" not in command
    syntax = subprocess.run(["bash", "-n"], input=command, text=True, capture_output=True)
    assert syntax.returncode == 0, syntax.stderr
    print("PASS: E2B compilation and staging use an isolated target directory.")
