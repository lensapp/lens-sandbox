#!/bin/sh
set -eu
[ "$#" = 4 ] || { echo 'usage: install.sh <source-app> <destination-app> <cli-directory> <version>' >&2; exit 1; }
install_source=$1
install_app=$2
install_bin=$3
install_version=$4
case "$install_app" in /*.app) ;; *) echo 'The app destination must be an absolute .app path.' >&2; exit 1;; esac
case "$install_bin" in /*) ;; *) echo 'The CLI directory must be an absolute path.' >&2; exit 1;; esac
[ ! -L "$install_app" ] || { echo 'Refusing to replace a symlinked app.' >&2; exit 1; }
install_team='@MACOS_TEAM_ID@'
printf '%s' "$install_team" | LC_ALL=C grep -Eq '^[A-Z0-9]{10}$' || { echo 'This installer has no configured signing team.' >&2; exit 1; }
codesign --verify --deep --strict -R "identifier \"run.lns.desktop\" and anchor apple generic and certificate leaf[subject.OU] = \"$install_team\"" "$install_source"
spctl --assess --type execute --verbose=2 "$install_source"
[ "$("$install_source/Contents/Helpers/lns" --version)" = "lns $install_version" ] || {
    echo 'The downloaded CLI version does not match the release.' >&2; exit 1;
}
[ -x "$install_source/Contents/Helpers/lns-service" ] || { echo 'The app has no service helper.' >&2; exit 1; }
mkdir -p "$(dirname -- "$install_app")" "$install_bin"
install_stage=$(mktemp -d "$(dirname -- "$install_app")/.lns-install.XXXXXX")
install_committed=0
install_finished=0
install_had_app=0
install_was_running=0
install_login=0
install_stopped=0
install_started=0
install_old_cli="$install_bin/lns"
if [ -x "$install_app/Contents/Helpers/lns" ]; then install_old_cli="$install_app/Contents/Helpers/lns"; fi
restore_install() {
    if [ "$install_finished" = 0 ] && [ "$install_committed" = 1 ]; then
        if [ "$install_started" = 1 ]; then
            if ! osascript -l JavaScript "$(dirname -- "$0")/quit.js" "$install_app" ||
                ! "$install_app/Contents/Helpers/lns" service stop; then
                echo "Could not stop the replacement; previous installation retained at $install_stage." >&2
                return 1
            fi
        fi
        rm -rf "$install_app"
        if [ "$install_had_app" = 1 ]; then mv "$install_stage/previous.app" "$install_app"; fi
        for install_helper in lns lns-service; do
            rm -f "$install_bin/$install_helper"
            if [ -e "$install_stage/$install_helper" ] || [ -L "$install_stage/$install_helper" ]; then
                mv "$install_stage/$install_helper" "$install_bin/$install_helper"
            fi
        done
    fi
    if [ "$install_finished" = 0 ] && [ "$install_stopped" = 1 ] && [ -x "$install_old_cli" ]; then
        if [ "$install_login" = 1 ]; then
            "$install_old_cli" service enable || echo 'Could not restore login startup; run lns service enable.' >&2
        elif [ "$install_was_running" = 1 ]; then
            "$install_old_cli" service start || echo 'Could not restart the previous service; run lns service start.' >&2
        fi
    fi
    rm -rf "$install_stage"
}
trap restore_install EXIT
trap 'exit 1' HUP INT TERM
ditto "$install_source" "$install_stage/LNS.app"
codesign --verify --deep --strict "$install_stage/LNS.app"
if [ -x "$install_old_cli" ]; then
    install_status=$("$install_old_cli" service status --format json) || { echo 'Cannot determine whether the existing service is running.' >&2; exit 1; }
    case "$(printf '%s' "$install_status" | tr -d '[:space:]')" in
        *'"running":true,'*|*'"running":true}'*) install_was_running=1;;
        *'"running":false,'*|*'"running":false}'*) install_was_running=0;;
        *) echo 'The existing service did not report a valid running state.' >&2; exit 1;;
    esac
fi
if [ -f "${HOME}/Library/LaunchAgents/run.lns.service.plist" ]; then install_login=1; fi
for install_helper in lns lns-service; do
    if [ -e "$install_bin/$install_helper" ] || [ -L "$install_bin/$install_helper" ]; then
        cp -P "$install_bin/$install_helper" "$install_stage/$install_helper"
    fi
done
osascript -l JavaScript "$(dirname -- "$0")/quit.js" "$install_app"
if [ "$install_login" = 1 ]; then
    "$install_old_cli" service disable
    install_stopped=1
elif [ "$install_was_running" = 1 ]; then
    "$install_old_cli" service stop
    install_stopped=1
fi

if [ -e "$install_app" ]; then mv "$install_app" "$install_stage/previous.app"; install_had_app=1; fi
install_committed=1
mv "$install_stage/LNS.app" "$install_app"
for install_helper in lns lns-service; do
    ln -s "$install_app/Contents/Helpers/$install_helper" "$install_stage/new-$install_helper"
    mv -f "$install_stage/new-$install_helper" "$install_bin/$install_helper"
done
unset LNS_SERVICE_BIN
if [ "$install_login" = 1 ]; then
    install_started=1
    "$install_app/Contents/Helpers/lns" service enable
elif [ "${LNS_NO_SERVICE:-0}" != 1 ] || [ "$install_was_running" = 1 ]; then
    install_started=1
    "$install_app/Contents/Helpers/lns" service start
fi
defaults write run.lns.desktop CLIInstallDirectory "$install_bin"
install_finished=1
printf 'Installed LNS %s at %s\n' "$install_version" "$install_app"
printf 'CLI: %s/lns\n' "$install_bin"
