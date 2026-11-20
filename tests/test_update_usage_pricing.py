import importlib.util
from pathlib import Path
import unittest
SPEC = importlib.util.spec_from_file_location("update_usage_pricing", Path(__file__).resolve().parents[1] / "scripts/update-usage-pricing.py")
pricing = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(pricing)


def gemini(name, inp, out, read):
    """A LiteLLM entry for Google's own API, Priority at 1.8 times."""
    return {f"gemini/{name}": {"litellm_provider": "gemini", "input_cost_per_token": inp, "output_cost_per_token": out,
                               "cache_read_input_token_cost": read, "input_cost_per_token_priority": inp * 1.8,
                               "output_cost_per_token_priority": out * 1.8,
                               "cache_read_input_token_cost_priority": read * 1.8}}


def rows(catalogue, model):
    return sorted((r for r in catalogue["rates"] if model in r["models"]), key=lambda r: r["effective_from"] or "")


class DatedListPriceTests(unittest.TestCase):
    def test_the_current_period_comes_from_litellm_and_the_next_from_the_rate_card(self):
        doc = gemini("gemini-4-carbon", 2e-06, 1e-05, 1e-07)
        before, after = rows(pricing.compile_catalogue(doc, "2026-11-20", "t"), "gemini-4-carbon")
        self.assertEqual((before["effective_until"], before["input_nanos_per_token"]), ("2026-12-01", 2000))
        self.assertEqual(before["source_url"], pricing.SOURCE)
        self.assertEqual((after["effective_from"], after["input_nanos_per_token"], after["cache_read_nanos_per_token"],
                          after["output_nanos_per_token"]), ("2026-12-01", 4000, 200, 20000))
        self.assertNotEqual(after["source_url"], pricing.SOURCE)
        # A cache rate LiteLLM leaves out keeps its ratio to input; Priority stays LiteLLM's.
        self.assertEqual((before["cache_write_nanos_per_token"], after["cache_write_nanos_per_token"]), (2000, 4000))
        self.assertEqual(before["tier_multipliers"], {"fast": [9, 5]})
        self.assertEqual(after["tier_multipliers"], {"fast": [9, 5]})

    def test_after_the_change_the_earlier_period_takes_the_rate_card_price(self):
        doc = gemini("gemini-4-carbon", 4e-06, 2e-05, 2e-07)
        before, after = rows(pricing.compile_catalogue(doc, "2026-12-05", "t"), "gemini-4-carbon")
        self.assertEqual((before["input_nanos_per_token"], before["output_nanos_per_token"]), (2000, 10000))
        self.assertEqual(after["source_url"], pricing.SOURCE)

    def test_litellm_must_agree_with_the_current_period(self):
        doc = gemini("gemini-4-carbon", 4e-06, 2e-05, 2e-07)
        with self.assertRaisesRegex(ValueError, "LiteLLM prices differ"):
            pricing.compile_catalogue(doc, "2026-11-20", "t")

    def test_a_rate_card_fallback_keeps_its_long_prompt_price(self):
        (row,) = rows(pricing.compile_catalogue({}, "2026-11-20", "t"), "gemini-3.1-pro")
        self.assertEqual((row["threshold_input_tokens"], row["above_threshold_input_nanos_per_token"],
                          row["above_threshold_read_nanos_per_token"], row["above_threshold_output_nanos_per_token"]),
                         (200000, 4000, 400, 18000))
        self.assertNotIn("above_threshold_write_nanos_per_token", row)


if __name__ == "__main__":
    unittest.main()
