#!/bin/sh
set -eu
deploy_test_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
deploy_test_tmp=$(mktemp -d)
trap 'rm -rf "$deploy_test_tmp"' EXIT HUP INT TERM
python3 - "$deploy_test_root/.github/workflows/release-please.yml" "$deploy_test_tmp/configure.sh" <<'PY'
from pathlib import Path
import sys
lines = Path(sys.argv[1]).read_text().splitlines()
start = lines.index('      - name: Configure the trusted macOS signing team')
for index in range(start + 1, len(lines)):
    if lines[index].startswith('        run: '):
        command = lines[index].removeprefix('        run: ')
        if command == '|':
            body = []
            for line in lines[index + 1:]:
                if line and not line.startswith('          '):
                    break
                body.append(line[10:])
            command = '\n'.join(body)
        Path(sys.argv[2]).write_text(command + '\n')
        break
else:
    raise AssertionError('installer configuration step missing')
PY
mkdir -p "$deploy_test_tmp/checkout/scripts/lns-install"
deploy_test_script="$deploy_test_tmp/checkout/scripts/lns-install/lns-install.sh"
printf '#!/bin/bash\necho legacy installer\n' > "$deploy_test_script"
cp "$deploy_test_script" "$deploy_test_tmp/expected"
(cd "$deploy_test_tmp/checkout" && ALLOW_LEGACY_INSTALLER=true MACOS_TEAM_ID='' bash "$deploy_test_tmp/configure.sh")
cmp "$deploy_test_script" "$deploy_test_tmp/expected"
echo 'PASS: explicit rollback preserves a pre-native installer without its absent helper or secrets'
if (cd "$deploy_test_tmp/checkout" && ALLOW_LEGACY_INSTALLER=false MACOS_TEAM_ID='' bash "$deploy_test_tmp/configure.sh"); then
    echo 'FAIL: normal publication bypassed signing configuration' >&2; exit 1
fi
echo 'PASS: normal publication requires signing configuration'
printf "team='@MACOS_TEAM_ID@'\n" > "$deploy_test_script"
if (cd "$deploy_test_tmp/checkout" && ALLOW_LEGACY_INSTALLER=true MACOS_TEAM_ID=TESTTEAM01 bash "$deploy_test_tmp/configure.sh"); then
    echo 'FAIL: a native template without its helper was treated as legacy' >&2; exit 1
fi
cp "$deploy_test_root/scripts/configure-macos-signing.py" "$deploy_test_tmp/checkout/scripts/"
for deploy_test_contents in "team='@MACOS_TEAM_ID@'" 'missing placeholder'; do
    printf '%s\n' "$deploy_test_contents" > "$deploy_test_script"
    if (cd "$deploy_test_tmp/checkout" && ALLOW_LEGACY_INSTALLER=true MACOS_TEAM_ID='' bash "$deploy_test_tmp/configure.sh"); then
        echo 'FAIL: native rollback accepted absent signing configuration' >&2; exit 1
    fi
done
if (cd "$deploy_test_tmp/checkout" && ALLOW_LEGACY_INSTALLER=true MACOS_TEAM_ID=TESTTEAM01 bash "$deploy_test_tmp/configure.sh"); then
    echo 'FAIL: native rollback accepted a missing placeholder' >&2; exit 1
fi
echo 'PASS: native rollback keeps strict signing validation'
printf "team='@MACOS_TEAM_ID@'\n" > "$deploy_test_script"
(cd "$deploy_test_tmp/checkout" && ALLOW_LEGACY_INSTALLER=true MACOS_TEAM_ID=TESTTEAM01 bash "$deploy_test_tmp/configure.sh")
[ "$(cat "$deploy_test_script")" = "team='TESTTEAM01'" ]
echo 'PASS: native rollback embeds the configured team'
echo "Results: 4 passed, 0 failed"
