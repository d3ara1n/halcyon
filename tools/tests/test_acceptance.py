"""验收入口合同：选择不暗加负载；进程退出、日志完整性和清理共同决定结果。

使用普通短命子进程验证宿主 runner，不启动 QEMU，不模拟内核行为。
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import acceptance

SUCCESS = """acceptance suite: memory, mode=0
kernel acceptance completed: memory
formation tasks complete
formation stopped: all instances collected
init: formation complete, submitting Shutdown
system reset accepted: action Shutdown, reason Requested
"""


class AcceptanceTests(unittest.TestCase):
    def setUp(self):
        (ROOT / "artifacts").mkdir(exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(prefix="runner-test-", dir=ROOT / "artifacts")
        self.root = Path(self.directory.name)
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("ERHINO_", "QEMU_ACCEPTANCE_"))}
        self.env.update(ERHINO_ACCEPTANCE_SUITE="memory", ERHINO_ACCEPTANCE_MODE="0")

    def tearDown(self):
        self.directory.cleanup()

    def argv(self, source, allow=False, timeout=5):
        return [sys.executable, str(ROOT / "tools/qemu-acceptance.py"),
                "--logs", str(self.root / "logs"), "--timeout", str(timeout),
                *(["--allow-reset-failure"] if allow else []),
                "--", sys.executable, "-u", "-c", source]

    def run_guest(self, text, exit_code=0, allow=False):
        result = subprocess.run(self.argv(f"import sys; print({text!r}, flush=True); sys.exit({exit_code})", allow),
                                cwd=ROOT, env=self.env, capture_output=True, text=True, timeout=15)
        evidence = json.loads(next((self.root / "logs").glob("*/result.json")).read_text())
        return result, evidence

    def test_explicit_scope(self):
        self.assertEqual(acceptance.select("memory", None), [("memory", "basic")])
        for key in acceptance.select("all", None):
            route = acceptance.ROUTES[key]
            self.assertEqual(route.observer, "normal")
            self.assertEqual(route.mode, 0)
            self.assertEqual(route.model, "virt")
        with self.assertRaises(ValueError):
            acceptance.select("memory", "misspelled")
        with self.assertRaises(ValueError):
            acceptance.command(("memory", "fault"), "release")

    def test_no_selection_only_lists(self):
        result = subprocess.run([sys.executable, str(ROOT / "tools/acceptance.py")],
                                env={**self.env, "PATH": ""}, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Usage:", result.stdout)

    def test_just_output_preserves_argument_boundaries(self):
        output = self.root / "argument.txt"
        payload = "acceptance suite-memory; literal $HOME"
        result = subprocess.run(
            ["just", "output", "argument-boundaries", "test", sys.executable, "-c",
             "import sys; from pathlib import Path; Path(sys.argv[1]).write_text(sys.argv[2])",
             str(output), payload], cwd=ROOT, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(output.read_text(), payload)
        # 此用例的包装日志没有验收现场价值，随隔离数据一起收掉。
        for line in result.stdout.splitlines():
            if "; log: " in line:
                (ROOT / line.split("; log: ", 1)[1]).unlink(missing_ok=True)

    def test_complete_success(self):
        result, evidence = self.run_guest(SUCCESS)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(evidence["status"], "passed")

    def test_missing_completion_fails(self):
        result, evidence = self.run_guest(SUCCESS.replace("kernel acceptance completed: memory\n", ""))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing anchors", evidence["error"])

    def test_wrong_suite_fails(self):
        result, _ = self.run_guest(SUCCESS.replace("memory", "ipc"))
        self.assertNotEqual(result.returncode, 0)

    def test_panic_after_success_fails(self):
        result, evidence = self.run_guest(SUCCESS + "Kernel panicking: test failure\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("guest failure", evidence["error"])

    def test_legacy_failed_marker_fails(self):
        result, evidence = self.run_guest(SUCCESS + "enumerate convergence FAILED: pid not monotonic\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("guest failure", evidence["error"])

    def test_transport_requires_pm_cleanup(self):
        self.env["ERHINO_ACCEPTANCE_SUITE"] = "transport"
        result, evidence = self.run_guest(SUCCESS.replace("memory", "transport"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("pm: active mailbox stop and refund passed", evidence["error"])

    def test_nonzero_exit_fails(self):
        result, evidence = self.run_guest(SUCCESS, 7)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("status 7", evidence["error"])

    def test_reboot_after_success_fails(self):
        result, _ = self.run_guest(SUCCESS + "system reset accepted: action Reboot, reason SystemFailure\n")
        self.assertNotEqual(result.returncode, 0)

    def test_sifive_harvest(self):
        result, evidence = self.run_guest(SUCCESS + "system reset failed: NotSupported\n", allow=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(evidence["harvested_after_reset_failure"])

    def test_reset_failure_without_completion_fails(self):
        result, _ = self.run_guest("system reset failed: NotSupported\n", allow=True)
        self.assertNotEqual(result.returncode, 0)

    def test_timeout_cleans_child(self):
        self.check_cleanup(interrupt=False)

    def test_interrupt_cleans_child(self):
        self.check_cleanup(interrupt=True)

    def check_cleanup(self, interrupt):
        pidfile = self.root / "child.pid"
        source = f"import os,time; from pathlib import Path; Path({str(pidfile)!r}).write_text(str(os.getpid())); time.sleep(30)"
        child_pid = None
        process = subprocess.Popen(self.argv(source, timeout=5 if interrupt else 0.3),
                                   cwd=ROOT, env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 5
            while not pidfile.exists() and time.monotonic() < deadline and process.poll() is None:
                time.sleep(0.01)
            self.assertTrue(pidfile.exists())
            child_pid = int(pidfile.read_text())
            if interrupt:
                process.send_signal(signal.SIGTERM)
            output, error = process.communicate(timeout=15)
            self.assertNotEqual(process.returncode, 0, output + error)
            with self.assertRaises(ProcessLookupError):
                os.kill(child_pid, 0)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            for stream in (process.stdout, process.stderr):
                stream.close()
            if child_pid:
                try:
                    os.kill(child_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass


if __name__ == "__main__":
    unittest.main()
