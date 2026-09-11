import pathlib
import shutil
import subprocess
import sys
import tempfile

checker = pathlib.Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix="lns icon smoke ") as directory:
    bundle = pathlib.Path(directory) / "Relocated LNS.app"
    shutil.copytree(sys.argv[2], bundle)
    executable = bundle / "Contents/MacOS/LNS"
    shutil.copy2(checker, executable)
    subprocess.run(["codesign", "--force", "--sign", "-", str(bundle)], check=True, timeout=20)
    subprocess.run([str(executable)], cwd=directory, check=True, timeout=20)
    (bundle / "Contents/Resources/lnsTemplate.png").unlink()
    missing = subprocess.run([str(executable)], cwd=directory, capture_output=True, text=True, timeout=20)
    assert missing.returncode == 1 and "lnsTemplate.png" in missing.stderr, "Missing menu-bar asset was not reported"
    print("PASS: missing menu-bar asset is reported explicitly")
