import hashlib
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import time


service_binary, client_binary = (str(Path(path).resolve()) for path in sys.argv[1:])
service_path = Path(service_binary)
assert service_path.parent.name == "Helpers" and service_path.parent.parent.name == "Contents", "smoke requires a bundled service"
app_bundle = service_path.parents[2]
assert app_bundle.suffix == ".app", "smoke requires an app bundle"
with tempfile.TemporaryDirectory(prefix="lns-ui-", dir="/tmp") as directory:
    root = Path(directory)
    connector = root / "connector.yaml"
    connector.write_text(json.dumps({
        "apiVersion": "lns.run/v1", "kind": "connector", "name": "issues",
        "spec": {"description": "Work with projects and issues.", "serves": ["api.issues.example"], "methods": [{"name": "public"}]},
    }))
    code_connector = root / "code-connector.yaml"
    code_connector.write_text(json.dumps({
        "apiVersion": "lns.run/v1", "kind": "connector", "name": "component-smoke",
        "spec": {"serves": ["api.fixture.example"], "methods": [{"name": "sign-in", "auth": {
            "kind": "code", "component": "./asking.wasm", "outputs": ["access_token", "resumed"], "hosts": []}}]},
    }))
    shutil.copyfile(Path(__file__).resolve().parents[3] / "crates/lns-service/tests/fixtures/asking.wasm", root / "asking.wasm")
    run = "aa010000000000000000000000000000"
    entry = hashlib.sha256(b"d\x01quiet_river\x01example.com\x01false").hexdigest()[:16]
    run_dir = root / "runs" / run
    run_dir.mkdir(parents=True)
    definition = {"apiVersion": "lns.run/v1", "kind": "sandbox", "name": "reviewer",
                  "spec": {"image": "alpine:3.20", "tools": ["node@20"]}}
    (root / "lns.yaml").write_text(json.dumps(definition))
    (root / "tools.yaml").write_text(json.dumps({"apiVersion": "lns.run/v1", "kind": "mixin", "name": "tools", "spec": {"tools": ["node@22"]}}))
    (run_dir / "record.json").write_text(json.dumps({
        "version": 1, "run_id": run, "name": "quiet_river",
        "args": {"image": "alpine:3.20", "cpus": 2, "mem": 1024, "cmd": ["sh"], "debug": False},
        "descriptor_sha256": "sha256:" + "a" * 64, "layer_digests": [],
        "image": "alpine:3.20", "command": "sh", "created_at": "2026-09-09T12:00:00Z",
        "finished_at": "2026-09-09T12:01:00Z", "exit_code": 0,
        "resolved_document": json.dumps(definition),
    }))
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
    previous = hashlib.sha256((audit_dir / "audit.jsonl").read_bytes().rstrip(b"\n")).hexdigest()
    with (audit_dir / "audit.jsonl").open("a") as audit:
        audit.write(json.dumps({
            "message": "oversized row", "payload": "x" * (1024 * 1024),
            "prev_hash": previous,
            "unmapped": {"lns_kind": "launch", "lns_run": run, "lns_ts": "2026-09-09T12:00:01Z"},
        }) + "\n")
    address = str(root / "service.sock")
    environment = dict(os.environ, LNS_HOME=directory, LNS_SOCKET_PATH=address, LNS_HEADLESS="1", LNS_NO_UPDATE_CHECK="1")
    update_environment = dict(environment, HTTP_PROXY="http://127.0.0.1:9", HTTPS_PROXY="http://127.0.0.1:9", ALL_PROXY="http://127.0.0.1:9", NO_PROXY="")
    cli_binary = Path(service_binary).with_name("lns")
    original_helpers = {path: hashlib.sha256(path.read_bytes()).hexdigest() for path in [cli_binary, Path(service_binary)]}
    update = subprocess.run([str(cli_binary), "update", "--force"], env=update_environment, capture_output=True, text=True, timeout=10)
    assert update.returncode != 0 and "fetching" in update.stderr, "bundled updater did not report the failed release download: " + update.stderr
    assert all(hashlib.sha256(path.read_bytes()).hexdigest() == digest for path, digest in original_helpers.items()), "failed download changed the installed helpers"
    dry_run = subprocess.run([str(cli_binary), "update", "--dry-run"], env=update_environment, capture_output=True, text=True, timeout=10)
    assert dry_run.returncode == 0, "update dry-run contacted the network: " + dry_run.stderr
    print("PASS: failed app update preserves helpers; dry-run works offline")
    with (root / "service.log").open("w+") as log:
        service = subprocess.Popen([service_binary], env=environment, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 10
            while not Path(address).exists():
                assert service.poll() is None, "service exited before binding"
                assert time.monotonic() < deadline, "service startup timed out"
                time.sleep(0.05)
            client = subprocess.run([client_binary, address, entry, str(connector)], capture_output=True, text=True, timeout=30)
            if client.returncode != 0:
                errors = [line for line in client.stderr.splitlines() if "Fatal error" in line or "Error raised" in line or "SMOKE FAILURE STAGE:" in line]
                detail = " | ".join(errors or client.stderr.splitlines()[:8])
                raise RuntimeError(f"Swift client exited {client.returncode}: {detail}")
            print(client.stdout, end="")
            assert service.wait(timeout=5) == 0
            assert "example.com" in (run_dir / "decisions.yaml").read_text(), "removal lost the policy decision"
            assert "example.com" in (root / "saved-reviewer.yaml").read_text(), "saved definition lost the live decision"
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

    relocated = root / "relocated/LNS.app"
    shutil.copytree(app_bundle, relocated)
    isolated_home = root / "user-home"
    isolated_home.mkdir()
    relocated_cli = str(relocated / "Contents/Helpers/lns")
    relocated_environment = dict(environment, HOME=str(isolated_home))
    with (root / "interface.log").open("w") as interface_log:
        interface = subprocess.Popen([str(relocated / "Contents/MacOS/LNS")], env=relocated_environment,
                                     stdout=interface_log, stderr=interface_log)
        try:
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                assert interface.poll() is None, "the relocated interface exited before uninstall"
                status = subprocess.run([relocated_cli, "service", "status", "--format", "json"],
                                        env=relocated_environment, capture_output=True, text=True, timeout=5)
                if status.returncode == 0 and json.loads(status.stdout).get("running"):
                    break
                time.sleep(0.1)
            else:
                raise AssertionError("the relocated interface did not start its service")
            uninstall = subprocess.run([relocated_cli, "uninstall", "--yes"],
                                       env=relocated_environment, capture_output=True, text=True, timeout=30)
            assert uninstall.returncode == 0, "native uninstall failed: " + uninstall.stderr
            interface.wait(timeout=5)
        finally:
            if interface.poll() is None:
                interface.terminate()
                interface.wait(timeout=5)
            if Path(relocated_cli).exists():
                stopped = subprocess.run([relocated_cli, "service", "stop"], env=relocated_environment,
                                         capture_output=True, text=True, timeout=10)
                assert stopped.returncode == 0, "failed to clean up the relocated service: " + stopped.stderr
    assert not relocated.exists(), "native uninstall left the app behind"
    assert (run_dir / "decisions.yaml").exists(), "native uninstall removed retained run data"
    assert Path(service_binary).exists(), "native uninstall removed a different app copy"
    print("PASS: native uninstall quits the running interface, removes only its app copy, and retains run data")
