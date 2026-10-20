import importlib.util
from pathlib import Path
import unittest
SPEC = importlib.util.spec_from_file_location("cache_analysis", Path(__file__).resolve().parents[1] / "scripts/cache-analysis.py")
analysis = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(analysis)

class CacheAnalysisTests(unittest.TestCase):
    def test_ttl_starts_at_request_start_and_censored_writes_are_excluded(self):
        def row(t, write=0, read=0, account="secret-account"):
            return dict(source="proxy", event_at_ms=t*1000, session_id="secret-session", account_id=account,
                        tokens=dict(cache_write=write, cache_read=read))
        result=analysis.analyze([row(0,100),row(301,100,100),row(302,100,100),row(700,100)],300)
        self.assertEqual(result["counts"]["candidate_unused_writes"],2)
        self.assertEqual(result["counts"]["right_censored_writes"],1)
        self.assertNotIn("secret",str(result))
    def test_other_accounts_cannot_prove_reuse(self):
        data=[dict(source="proxy", event_at_ms=t, session_id="s", account_id=a,tokens=tokens)
              for t,a,tokens in [(0,"a",{"cache_write":100}),(100,"b",{"cache_read":100})]]
        result=analysis.analyze(data,as_of_ms=400000)
        self.assertEqual(result["counts"]["candidate_unused_writes"],1)
