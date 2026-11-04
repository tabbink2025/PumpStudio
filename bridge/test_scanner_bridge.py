"""Offline: scanner bridge against a fake serial port. No hardware."""
import threading
import time
import unittest

from scanner_bridge import Scanner


class FakeSerial:
    """Answers the trigger 'BA' with the queued answers (one per trigger); records everything written."""
    def __init__(self, answers):
        self.answers = list(answers)
        self.written = b""
        self.buffer = b""

    def reset_input_buffer(self):
        self.buffer = b""

    def write(self, data):
        self.written += data
        if data == b"BA" and self.answers:
            self.buffer += self.answers.pop(0)

    def read(self, n):
        if not self.buffer:
            time.sleep(0.01)
            return b""
        out, self.buffer = self.buffer[:n], self.buffer[n:]
        return out

    def close(self):
        pass


class ScannerBridgeTests(unittest.TestCase):
    def make(self, answers):
        fake = FakeSerial(answers)
        s = Scanner(serial_factory=lambda a: fake)
        s.connect(dict(port="COM9", baud=9600))
        return s, fake

    def test_code_then_noread(self):
        s, fake = self.make([b"\x020000000012\x03\r\n", b"\x02\x15"])
        out = s.read(dict(on=list(b"BA"), off=list(b"B"), timeout=2))
        self.assertEqual(bytes(out["bytes"]), b"\x020000000012\x03\r\n"[:len(out["bytes"])])
        self.assertIn(0x03, out["bytes"])
        out = s.read(dict(on=list(b"BA"), off=list(b"B"), timeout=2))
        self.assertEqual(out["bytes"], [0x02, 0x15])
        self.assertEqual(fake.written, b"BABBAB")

    def test_timeout_and_stop(self):
        s, _ = self.make([])
        start = time.monotonic()
        out = s.read(dict(on=list(b"BA"), off=list(b"B"), timeout=0.3))
        self.assertEqual(out["bytes"], [])
        self.assertLess(time.monotonic() - start, 0.6)
        threading.Timer(0.2, s.stop).start()
        start = time.monotonic()
        out = s.read(dict(on=list(b"BA"), off=list(b"B"), timeout=30))
        self.assertTrue(out["stopped"])
        self.assertLess(time.monotonic() - start, 0.6)


if __name__ == "__main__":
    unittest.main()
