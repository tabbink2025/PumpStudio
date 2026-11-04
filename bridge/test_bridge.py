"""Offline checks: never import CAN or open COM ports."""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


class ProtocolTests(unittest.TestCase):
    def test_pipe_protocol_without_hardware(self):
        with tempfile.TemporaryDirectory() as output:
            requests = [dict(id=1, op="ping"), dict(id=2, op="weight"), dict(id=3, op="ping")]
            result = subprocess.run([sys.executable, str(Path(__file__).with_name("hardware_bridge.py")),
                                     "--repo", output, "--output", output],
                                    input="".join(json.dumps(r)+"\n" for r in requests),
                                    capture_output=True, text=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stderr)
            rows = [json.loads(line) for line in result.stdout.splitlines()]
            self.assertEqual([r["id"] for r in rows], [1,2,3])
            self.assertTrue(rows[0]["ok"])
            self.assertFalse(rows[0]["result"]["hardware_open"])
            self.assertFalse(rows[1]["ok"])
            self.assertIn("niet verbonden", rows[1]["error"])
            self.assertFalse(rows[2]["result"]["hardware_open"])

    def test_bad_input_does_not_start_hardware(self):
        with tempfile.TemporaryDirectory() as output:
            result = subprocess.run([sys.executable, str(Path(__file__).with_name("hardware_bridge.py")),
                                     "--repo", output, "--output", output],
                                    input="not json\n{}\n",capture_output=True,text=True,timeout=15)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(len(result.stdout.splitlines()),2)
            self.assertTrue(all(not json.loads(line)["ok"] for line in result.stdout.splitlines()))


if __name__ == "__main__":
    unittest.main()
