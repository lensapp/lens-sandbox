#!/bin/sh
set -eu
native_ci_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
python3 - "$native_ci_root" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1])
workflows = list((root / '.github/workflows').glob('*.yml'))
assert sum(path.read_text().count('make -C clients/macos verify') for path in workflows) == 1, 'native verification must have one owner'
ci = (root / '.github/workflows/ci.yml').read_text()
job = ci.split('\n  macos-ui:\n', 1)[1].split('\n  test:', 1)[0]
assert 'timeout-minutes:' in job, 'native verification needs a timeout'
assert 'make -C clients/macos package' in job and 'make -C clients/macos smoke' in job, 'the verified native job must build and smoke-test its artifact'
for message in ['echo "One or more gates failed:', 'echo "All gates passed (']:
    line = next(line for line in ci.splitlines() if message in line)
    assert "macos-ui=${{ needs['macos-ui'].result }}" in line, 'the aggregate result must name macos-ui'
print('PASS: one required native job owns verification, package, smoke, timeout, and status reporting')
PY
echo "Results: 1 passed, 0 failed"
