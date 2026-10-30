#!/usr/bin/env python3
"""Compile the supported model families using T3 Code 0.1.3 pricing rules.

Accepts LiteLLM's public document or T3's cached {fetchedAtMs, document} wrapper.
No runtime network dependency. Keep the input hash and date with the release.
"""
import argparse
import datetime
from fractions import Fraction
import hashlib
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SOURCE = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json"


def token_rates(entry, suffix="", standard=None):
    def number(key):
        value = entry.get(key + suffix)
        return Fraction(str(value)) if isinstance(value, (int, float)) and not isinstance(value, bool) else None

    inp, out = number("input_cost_per_token"), number("output_cost_per_token")
    if inp is None or out is None:
        return None
    cache = []
    for i, key in enumerate(("cache_read_input_token_cost", "cache_creation_input_token_cost"), 1):
        value = number(key)
        cache.append(value if value is not None else standard[i] / standard[0] * inp if standard and standard[0] > 0 else inp)
    return (inp, *cache, out)


def rate_table(document):
    table = {}
    for name, entry in document.items():
        if not isinstance(entry, dict):
            continue
        standard = token_rates(entry)
        if standard is None:
            continue
        specific = entry.get("provider_specific_entry") or {}
        multiple = specific.get("fast") if isinstance(specific, dict) else None
        fast = tuple(n * Fraction(str(multiple)) for n in standard) if isinstance(multiple, (int, float)) and multiple > 0 else token_rates(entry, "_priority", standard)
        table[name.strip().lower()] = (standard, fast, token_rates(entry, "_ultrafast", standard))
    candidates = {}
    for name, rate in table.items():
        bare = name.rsplit("/", 1)[-1]
        if bare == name or bare in table:
            continue
        if bare not in candidates:
            candidates[bare] = rate
        elif candidates[bare] is not None:
            held = candidates[bare]
            merged = (held[0], held[1] or rate[1], held[2] or rate[2])
            candidates[bare] = merged if held[0] == rate[0] and all(rate[i] is None or rate[i] == merged[i] for i in (1, 2)) else None
    table.update({name: rate for name, rate in candidates.items() if rate is not None})
    return table


def compile_catalogue(document, date, version):
    provider = json.loads((ROOT / "src/usage/rates.json").read_text())
    families = {name: row["provider"] for row in provider["rates"] for name in row["models"]}
    table = rate_table(document)
    groups = {}
    for name, (standard, fast, ultra) in table.items():
        # Fusebox's bundled scope is direct OpenAI/Anthropic model families.
        # Cloud reseller price cards are distinct and need explicit overrides.
        if "/" in name and name.split("/", 1)[0] not in ("openai", "anthropic"):
            continue
        bare = name.rsplit("/", 1)[-1]
        family = re.sub(r"-\d{8}$", "", bare)
        if family not in families:
            continue
        nanos = tuple(n * 1_000_000_000 for n in standard)
        if any(n.denominator != 1 or n < 0 for n in nanos):
            raise ValueError(f"{name}: unsupported sub-nanodollar rate")
        tiers = {}
        for speed, rates in (("fast", fast), ("ultrafast", ultra)):
            if rates is None:
                continue
            ratios = {b / a for a, b in zip(standard, rates) if a}
            if len(ratios) != 1 or any(not a and b for a, b in zip(standard, rates)):
                raise ValueError(f"{name}: nonuniform speed rates require a rate schema update")
            multiple = ratios.pop()
            tiers[speed] = [multiple.numerator, multiple.denominator]
        key = (families[family], nanos, json.dumps(tiers, sort_keys=True))
        if key not in groups:
            groups[key] = {"provider": key[0], "models": [], "effective_from": None, "effective_until": None,
                           "source_url": SOURCE, "input_nanos_per_token": int(nanos[0]),
                           "cache_read_nanos_per_token": int(nanos[1]), "cache_write_nanos_per_token": int(nanos[2]),
                           "cache_write_5m_nanos_per_token": None, "cache_write_1h_nanos_per_token": None,
                           "output_nanos_per_token": int(nanos[3]), "tier_multipliers": tiers}
        groups[key]["models"].append(name)
    rates = list(groups.values())
    present = {name for row in rates for name in row["models"]}
    # Preserve existing published model support when LiteLLM has no matching ID.
    # These rows retain their official source URL, making the fallback auditable.
    for row in provider["rates"]:
        missing = [name for name in row["models"] if name not in present]
        if missing:
            rates.append({"provider": row["provider"], "models": missing, "effective_from": None,
                          "effective_until": None, "source_url": row["source_url"],
                          "input_nanos_per_token": row["input_nanos_per_token"],
                          "cache_read_nanos_per_token": row.get("cache_read_nanos_per_token"),
                          "cache_write_nanos_per_token": row.get("cache_write_5m_nanos_per_token"),
                          "cache_write_5m_nanos_per_token": None, "cache_write_1h_nanos_per_token": None,
                          "output_nanos_per_token": row["output_nanos_per_token"], "tier_multipliers": {}})
    for row in rates:
        row["models"].sort()
    rates.sort(key=lambda row: (row["provider"], row["models"][0]))
    return {"version": version, "verified_at": date, "calculation": "t3", "rates": rates, "currency": "USD"}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("source", type=Path)
    ap.add_argument("--date", required=True)
    ap.add_argument("--version", required=True)
    ap.add_argument("--output", type=Path, default=ROOT / "src/usage/rates-t3.json")
    args = ap.parse_args()
    datetime.date.fromisoformat(args.date)
    raw = args.source.read_bytes()
    parsed = json.loads(raw)
    catalogue = compile_catalogue(parsed.get("document", parsed), args.date, args.version)
    args.output.write_text(json.dumps(catalogue, indent=2) + "\n")
    print(json.dumps({"source_sha256": hashlib.sha256(raw).hexdigest(), "rates": len(catalogue["rates"]),
                      "model_ids": sum(len(row["models"]) for row in catalogue["rates"])}))


if __name__ == "__main__":
    main()
