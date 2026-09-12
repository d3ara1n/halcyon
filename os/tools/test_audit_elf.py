"""ELF 审计的布局派生阈值与显式覆盖约束。"""

import contextlib
import importlib.util
import io
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("audit_elf", Path(__file__).with_name("audit_elf.py"))
AUDIT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUDIT)


class AuditFrameLimitTests(unittest.TestCase):
    def run_audit(self, frame, guard, arguments=()):
        header = "ELF64 RISC-V\nFlags: 0x1, RVC, soft-float ABI\n"
        disasm = f"""Disassembly of section .text.ctx_fp:
Disassembly of section .text:
1000 <function>:
 1000: 1234 li t0,{frame}
 1004: 1234 sub sp,sp,t0
 1008: 1234 add sp,sp,t0
"""
        symbols = f"1: {guard:016x} 0 NOTYPE GLOBAL DEFAULT ABS STACK_GUARD\n"
        output = io.StringIO()
        with patch.object(AUDIT.sys, "argv", ["audit_elf.py", "kernel.elf", *arguments]), \
                patch.object(AUDIT.subprocess, "run", side_effect=[
                    SimpleNamespace(stdout=header), SimpleNamespace(stdout=disasm),
                    SimpleNamespace(stdout=symbols),
                ]), contextlib.redirect_stdout(output):
            AUDIT.main()
        return output.getvalue()

    def test_default_uses_guard_instead_of_old_arbitrary_threshold(self):
        self.assertIn("audit passed", self.run_audit(0x2890, 0x3000))
        self.assertIn("audit passed", self.run_audit(0x3000, 0x3000))
        self.assertIn("audit passed", self.run_audit(0x4000, 0x4000))

    def test_frame_larger_than_guard_is_rejected(self):
        with self.assertRaises(SystemExit) as error:
            self.run_audit(0x3010, 0x3000)
        self.assertEqual(error.exception.code, 1)

    def test_explicit_limit_cannot_exceed_guard(self):
        with self.assertRaises(SystemExit):
            self.run_audit(0x1000, 0x3000, ("--max-frame", "0x4000"))

    def test_explicit_smaller_limit_is_enforced(self):
        with self.assertRaises(SystemExit):
            self.run_audit(0x2890, 0x3000, ("--max-frame", "0x2800"))

    def test_local_labels_do_not_split_frame_or_constant_tracking(self):
        disasm = """1000 <function>:
 1000: 1234 lui t0,0x3
1004 <.Lpcrel_hi0>:
 1004: 1234 addi t0,t0,16
 1008: 1234 sub sp,sp,t0
 100c: 1234 add sp,sp,t0
1010 <next_function>:
 1010: 1234 addi sp,sp,-32
 1014: 1234 addi sp,sp,32
"""
        frames = AUDIT.scan_frames(disasm)
        self.assertEqual([(name, size) for name, size, _ in frames],
                         [("function", 0x3010), ("next_function", 32)])

    def test_nonpositive_limit_is_rejected(self):
        with self.assertRaises(SystemExit):
            self.run_audit(0x1000, 0x3000, ("--max-frame", "0"))


if __name__ == "__main__":
    unittest.main()
