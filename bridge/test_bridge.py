"""Unit tests for the bridge's settlement polling.

Run with `python3 -m unittest discover -s bridge`.
"""
import os
import sys
import unittest

os.environ.setdefault("GK_KEY", "gk_test")
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bridge  # noqa: E402 - path must be set before import


PAYLOAD = {
    "to": "0xd3f7F985F14f1942Fb09e5735e5499FEFF56E80b",
    "data": "0x9c98c06e",
    "value": "0x0",
    "chain_id": 11155111,
    "estimated_gas": 210000,
    "valid_until_block": 9000000,
}


class Poll(unittest.TestCase):
    """Drives await_payload against scripted GET /tasks/<id> responses."""

    def setUp(self):
        self._real = bridge.http_json
        self.clock = 0.0
        self.slept = []

    def tearDown(self):
        bridge.http_json = self._real

    def scripted(self, *responses):
        """Serves each response in turn, repeating the last one indefinitely."""
        queue = list(responses)
        self.requested = []

        def fake(url, payload=None, headers=None, timeout=30):
            self.requested.append(url)
            return queue.pop(0) if len(queue) > 1 else queue[0]

        bridge.http_json = fake

    def now(self):
        return self.clock

    def sleep(self, seconds):
        self.slept.append(seconds)
        self.clock += seconds

    def await_payload(self, timeout=None):
        return bridge.await_payload("t1", timeout=timeout, now=self.now, sleep=self.sleep)

    def test_ready_returns_the_payload(self):
        self.scripted({"status": "ready", "payload": PAYLOAD})
        self.assertEqual(self.await_payload(), PAYLOAD)
        self.assertEqual(self.slept, [], "a ready task should not sleep")

    def test_polls_through_the_pending_states(self):
        self.scripted(
            {"status": "queued"},
            {"status": "processing"},
            {"status": "ready", "payload": PAYLOAD},
        )
        self.assertEqual(self.await_payload(), PAYLOAD)
        self.assertEqual(len(self.requested), 3)
        self.assertTrue(self.requested[0].endswith("/tasks/t1"))

    def test_failed_raises_with_the_routers_reason(self):
        self.scripted({"status": "failed", "error": "quorum not reached"})
        with self.assertRaises(RuntimeError) as caught:
            self.await_payload()
        self.assertIn("quorum not reached", str(caught.exception))

    def test_expired_raises(self):
        self.scripted({"status": "expired", "error": "ttl elapsed"})
        with self.assertRaisesRegex(RuntimeError, "expired"):
            self.await_payload()

    def test_terminal_failure_without_a_reason_still_raises(self):
        self.scripted({"status": "failed"})
        with self.assertRaisesRegex(RuntimeError, "no reason given"):
            self.await_payload()

    def test_ready_without_a_payload_is_an_error(self):
        """A ready task must carry a payload; reporting success without one would
        tell the caller an answer is submittable when nothing was rendered."""
        self.scripted({"status": "ready", "payload": None})
        with self.assertRaisesRegex(RuntimeError, "no payload"):
            self.await_payload()

    def test_unknown_status_is_not_treated_as_pending(self):
        """Guards against a new router status silently polling until timeout."""
        self.scripted({"status": "reticulating"})
        with self.assertRaisesRegex(RuntimeError, "unknown status"):
            self.await_payload()

    def test_timeout_while_pending(self):
        self.scripted({"status": "processing"})
        with self.assertRaises(TimeoutError):
            self.await_payload(timeout=12)
        self.assertLessEqual(self.clock, 12 + bridge.SETTLE_POLL_SECONDS)


if __name__ == "__main__":
    unittest.main()
