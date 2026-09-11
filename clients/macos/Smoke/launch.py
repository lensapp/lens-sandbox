import pathlib
import subprocess
import sys
import tempfile

with tempfile.TemporaryDirectory(prefix="lns launch ") as directory:
    bundle = pathlib.Path(directory) / "LNS.app"
    helper = bundle / "Contents" / "Helpers" / "lns"
    helper.parent.mkdir(parents=True)
    helper.write_text('''#!/bin/sh
set -eu
test "$#" = 4
test "$1" = run
test "$2" = --detach
test "$3" = --name=--debug
test "$4" = '/project with spaces/lns.yaml'
test "$LNS_SOCKET_PATH" = /private/test/service.sock
test "$LNS_HEADLESS" = 1
test "$PWD" = /
test -n "$LNS_SERVICE_BIN"
awk 'BEGIN { for (i = 0; i < 20000; i++) print "launch progress" }' >&2
printf '0123456789abcdef0123456789abcdef\\n'
printf 'final launch diagnostic\\n' >&2
exit "$SMOKE_EXIT"
''')
    helper.chmod(0o755)
    subprocess.run([str(pathlib.Path(sys.argv[1]).resolve()), str(bundle)], check=True, timeout=20)
