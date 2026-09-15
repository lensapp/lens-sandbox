import os
from pathlib import Path
import re
import sys


def main():
    team = os.environ.get('MACOS_TEAM_ID', '')
    if re.fullmatch(r'[A-Z0-9]{10}', team) is None:
        raise ValueError('MACOS_TEAM_ID must be a 10-character Apple Developer Team ID')
    paths = [Path(argument) for argument in sys.argv[1:]]
    if not paths:
        raise ValueError('provide the installer templates to configure')
    templates = [(path, path.read_text()) for path in paths]
    for path, template in templates:
        if template.count('@MACOS_TEAM_ID@') != 1:
            raise ValueError(f'{path} must contain exactly one signing team placeholder')
    for path, template in templates:
        path.write_text(template.replace('@MACOS_TEAM_ID@', team))
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, OSError) as error:
        sys.exit(str(error))
