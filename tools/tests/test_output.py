"""正式演示输出包装的 host 检查；不启动 QEMU。"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class OutputTests(unittest.TestCase):
    def test_just_output_preserves_argument_boundaries(self):
        (ROOT / "artifacts").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="output-test-", dir=ROOT / "artifacts") as directory:
            output = Path(directory) / "argument.txt"
            payload = "system argument; literal $HOME"
            result = subprocess.run(
                ["just", "output", "argument-boundaries", sys.executable, "-c",
                 "import sys; from pathlib import Path; Path(sys.argv[1]).write_text(sys.argv[2])",
                 str(output), payload], cwd=ROOT, env={**os.environ, "ERHINO_OUTPUT": "summary"},
                capture_output=True, text=True, timeout=5)
            try:
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(output.read_text(), payload)
            finally:
                for line in result.stdout.splitlines():
                    if "; log: " in line:
                        (ROOT / line.split("; log: ", 1)[1]).unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
