import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


service_binary, client_binary = (str(Path(path).resolve()) for path in sys.argv[1:])
with tempfile.TemporaryDirectory(prefix="lns-ui-", dir="/tmp") as directory:
    root = Path(directory)
    run = "aa010000000000000000000000000000"
    entry = hashlib.sha256(b"d\x01quiet_river\x01example.com\x01false").hexdigest()[:16]
    run_dir = root / "runs" / run
    run_dir.mkdir(parents=True)
    (run_dir / "approvals.json").write_text(json.dumps([{
        "id": entry, "sandbox": "quiet_river", "state": "withdrawn",
        "kind": {"kind": "destination", "destination": "example.com", "action": "CONNECT example.com:443", "raw": False},
    }]))
    audit_dir = root / "audit" / run
    audit_dir.mkdir(parents=True)
    (audit_dir / "audit.jsonl").write_text(json.dumps({
        "message": "launch alpine:3.20", "prev_hash": "0" * 64,
        "unmapped": {"lns_kind": "launch", "lns_run": run, "lns_microvm": "quiet_river", "lns_image": "alpine:3.20", "lns_ts": "2026-09-09T12:00:00Z"},
    }) + "\n")
    address = str(root / "service.sock")
    environment = dict(os.environ, LNS_HOME=directory, LNS_SOCKET_PATH=address, LNS_HEADLESS="1", LNS_NO_UPDATE_CHECK="1")
    update_environment = dict(environment, HTTP_PROXY="http://127.0.0.1:9", HTTPS_PROXY="http://127.0.0.1:9", ALL_PROXY="http://127.0.0.1:9", NO_PROXY="")
    update = subprocess.run([str(Path(service_binary).with_name("lns")), "update", "--force"], env=update_environment, capture_output=True, text=True, timeout=10)
    assert update.returncode != 0 and "complete app" in update.stderr, "bundled updater did not refuse loose-binary replacement: " + update.stderr
    with (root / "service.log").open("w+") as log:
        service = subprocess.Popen([service_binary], env=environment, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 10
            while not Path(address).exists():
                assert service.poll() is None, "service exited before binding"
                assert time.monotonic() < deadline, "service startup timed out"
                time.sleep(0.05)
            client = subprocess.run([client_binary, address, entry], capture_output=True, text=True, timeout=30)
            if client.returncode != 0:
                errors = [line for line in client.stderr.splitlines() if "Fatal error" in line or "Error raised" in line]
                detail = " | ".join(errors or client.stderr.splitlines()[:8])
                raise RuntimeError(f"Swift client exited {client.returncode}: {detail}")
            print(client.stdout, end="")
            assert service.wait(timeout=5) == 0
            assert "example.com" in (run_dir / "decisions.yaml").read_text(), "removal lost the policy decision"
        except BaseException:
            log.flush()
            log.seek(0)
            sys.stderr.write(log.read())
            raise
        finally:
            if service.poll() is None:
                service.terminate()
                try:
                    service.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    service.kill()
                    service.wait(timeout=5)
