#!/usr/bin/env python3
"""Compile the supported model families using T3 Code's pricing rules (0.1.11).

T3 reads LiteLLM's standard, Fast (priority or a `fast` multiple) and Ultrafast
rates, a one-hour cache-write rate, and long-prompt rates that apply when a
request's whole prompt (uncached, cached and written) is above a threshold.
A bare model name takes the maker's own price over a cloud's or reseller's.

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
# LiteLLM prefixes for the model makers' own APIs, as in T3's usagePricing.ts.
FIRST_PARTY = {"anthropic", "deepseek", "gemini", "meta", "mistral", "openai", "xai"}
SOURCE = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json"
CONTEXT_TIER_KEY = re.compile(r"^input_cost_per_token_above_(\d{1,7})k_tokens(?:_priority|_ultrafast)?$")
SPEEDS = ("fast", "ultrafast")
# Rate tuples: (input, cache read, cache write, one-hour cache write, output).
INPUT, READ, WRITE, WRITE_1H, OUTPUT = range(5)
# The prices a dated rate card period sets; LiteLLM lists only today's.
LIST_PRICES = ("input_nanos_per_token", "cache_read_nanos_per_token", "output_nanos_per_token")
LONG_PROMPT = ("threshold_input_tokens", "above_threshold_input_nanos_per_token",
               "above_threshold_read_nanos_per_token", "above_threshold_output_nanos_per_token")


def token_rates(entry, suffix="", standard=None, output_fallback=None):
    """T3's readTokenRates: a missing cache rate keeps the standard rate's ratio to input."""
    def number(key):
        value = entry.get(key + suffix)
        return Fraction(str(value)) if isinstance(value, (int, float)) and not isinstance(value, bool) else None

    inp = number("input_cost_per_token")
    out = number("output_cost_per_token")
    if out is None:
        out = output_fallback
    if inp is None or out is None:
        return None

    def cache(key, field, fallback):
        value = number(key)
        if value is not None:
            return value
        return standard[field] / standard[INPUT] * inp if standard and standard[INPUT] > 0 else fallback

    write = cache("cache_creation_input_token_cost", WRITE, inp)
    read = cache("cache_read_input_token_cost", READ, inp)
    return (inp, read, write, cache("cache_creation_input_token_cost_above_1hr", WRITE_1H, write), out)


def speed_rates(entry, standard, suffix=""):
    """T3's readSpeedRates: a `fast` multiple scales the standard rates, else the priority rates."""
    specific = entry.get("provider_specific_entry")
    multiple = specific.get("fast") if isinstance(specific, dict) else None
    if isinstance(multiple, (int, float)) and not isinstance(multiple, bool) and multiple > 0:
        fast = tuple(n * Fraction(str(multiple)) for n in standard)
    else:
        fast = token_rates(entry, suffix + "_priority", standard)
    return {"standard": standard, "fast": fast, "ultrafast": token_rates(entry, suffix + "_ultrafast", standard)}


def context_tiers(entry, base):
    """T3's readContextTiers, highest threshold first."""
    thresholds = {int(m.group(1)) for m in map(CONTEXT_TIER_KEY.match, entry) if m}
    tiers = []
    for threshold in sorted(thresholds, reverse=True):
        suffix = f"_above_{threshold}k_tokens"
        standard = token_rates(entry, suffix, base, base[OUTPUT]) or base
        tiers.append((threshold * 1000, speed_rates(entry, standard, suffix)))
    return tiers


def merge_speeds(a, b):
    fast, ultra = a["fast"] or b["fast"], a["ultrafast"] or b["ultrafast"]
    if a["standard"] != b["standard"] or (b["fast"] or fast) != fast or (b["ultrafast"] or ultra) != ultra:
        return None
    return {"standard": a["standard"], "fast": fast, "ultrafast": ultra}


def merge_alias(a, b):
    """T3's mergeAliasRate: aliases of the same standing must agree, or the name stays unpriced."""
    speeds = merge_speeds(a, b)
    if speeds is None:
        return None
    if not a["tiers"] or not b["tiers"]:
        tiers = a["tiers"] or b["tiers"]
    elif len(a["tiers"]) != len(b["tiers"]):
        return None
    else:
        tiers = []
        for (ta, ra), (tb, rb) in zip(a["tiers"], b["tiers"]):
            merged = merge_speeds(ra, rb)
            if ta != tb or merged is None:
                return None
            tiers.append((ta, merged))
    return {**speeds, "tiers": tiers}


def rate_table(document):
    table = {}
    for name, entry in document.items():
        if not isinstance(entry, dict):
            continue
        standard = token_rates(entry)
        if standard is None:
            continue
        key = name.strip().lower()
        if key:
            table[key] = {**speed_rates(entry, standard), "tiers": context_tiers(entry, standard)}
    # A bare name takes the maker's own price over a cloud's or reseller's (T3 #28004);
    # entries of the same standing must agree, or the name stays unpriced.
    candidates = {}
    for name, rate in table.items():
        alias = name.rsplit("/", 1)[-1] if "/" in name else name.removeprefix("anthropic.")
        if not alias or alias == name or alias in table:
            continue
        first_party = "/" in name and name.split("/", 1)[0] in FIRST_PARTY
        held = candidates.get(alias)
        if held is None or (first_party and not held[1]):
            candidates[alias] = (rate, first_party)
        elif held[0] is not None and first_party == held[1]:
            candidates[alias] = (merge_alias(held[0], rate), first_party)
    table.update({name: rate for name, (rate, _) in candidates.items() if rate is not None})
    return table


def nanos(rates, name):
    out = tuple(n * 1_000_000_000 for n in rates)
    if any(n.denominator != 1 or n < 0 for n in out):
        raise ValueError(f"{name}: unsupported sub-nanodollar rate")
    return tuple(int(n) for n in out)


def multiple(standard, rates, name):
    """One ratio for every category, as Fusebox stores a speed as a multiplier."""
    ratios = {b / a for a, b in zip(standard, rates) if a}
    if len(ratios) != 1 or any(not a and b for a, b in zip(standard, rates)):
        raise ValueError(f"{name}: nonuniform speed rates require a rate schema update")
    ratio = ratios.pop()
    return [ratio.numerator, ratio.denominator]


def scaled(base, field, tier_input):
    """The code's fallback for an omitted long-prompt cache rate (T3's ratio rule)."""
    return Fraction(base[field] * tier_input, base[INPUT]) if base[INPUT] else None


def model_row(provider, name, rate):
    standard = rate["standard"]
    base = nanos(standard, name)
    tiers = {speed: multiple(standard, rate[speed], name) for speed in SPEEDS if rate[speed] is not None}
    row = {"provider": provider, "models": [name], "effective_from": None, "effective_until": None,
           "source_url": SOURCE, "input_nanos_per_token": base[INPUT],
           "cache_read_nanos_per_token": base[READ], "cache_write_nanos_per_token": base[WRITE],
           "cache_write_5m_nanos_per_token": None,
           # Only a one-hour rate that differs; the code falls back to the write rate.
           "cache_write_1h_nanos_per_token": base[WRITE_1H] if base[WRITE_1H] != base[WRITE] else None,
           "output_nanos_per_token": base[OUTPUT], "tier_multipliers": tiers}
    if len(rate["tiers"]) > 1:
        raise ValueError(f"{name}: several long-prompt thresholds require a rate schema update")
    for threshold, tier in rate["tiers"]:
        above = nanos(tier["standard"], name)
        row.update({"threshold_input_tokens": threshold,
                    "above_threshold_input_nanos_per_token": above[INPUT],
                    "above_threshold_read_nanos_per_token": above[READ],
                    "above_threshold_write_nanos_per_token": above[WRITE],
                    "above_threshold_output_nanos_per_token": above[OUTPUT]})
        # Omitted when the code's fallback gives the same rate: releases before 0.6
        # cannot read a long-prompt lifetime rate, and a downgrade drops its row.
        if scaled(base, WRITE_1H, above[INPUT]) != above[WRITE_1H]:
            row["above_threshold_write_1h_nanos_per_token"] = above[WRITE_1H]
        # T3 prices a long prompt at a speed with the tier's speed rate when it has
        # one, else with the base speed rate: those speeds skip the long-prompt rates.
        excluded = []
        for speed in SPEEDS:
            if tier[speed] is None:
                if rate[speed] is not None:
                    excluded.append(speed)
            elif rate[speed] is None or multiple(tier["standard"], tier[speed], name) != tiers[speed]:
                raise ValueError(f"{name}: long-prompt speed rates require a rate schema update")
        if excluded:
            row["long_context_excluded_tiers"] = excluded
    return row


def compile_catalogue(document, date, version):
    provider = json.loads((ROOT / "src/usage/rates.json").read_text())
    families = {name: row["provider"] for row in provider["rates"] for name in row["models"]}
    dated = {}
    for row in provider["rates"]:
        if row.get("effective_from") or row.get("effective_until"):
            for name in row["models"]:
                dated.setdefault(name, []).append(row)
    day = datetime.date.fromisoformat(date).isoformat()
    rows = []
    for name, rate in rate_table(document).items():
        # Fusebox's bundled scope is the makers' own model families. Bare names
        # take the maker's price (Google's from `gemini/`); cloud reseller price
        # cards are distinct and need explicit overrides.
        if "/" in name and name.split("/", 1)[0] not in ("openai", "anthropic"):
            continue
        bare = name.rsplit("/", 1)[-1]
        family = re.sub(r"-\d{8}$", "", bare)
        if family not in families:
            continue
        row = model_row(families[family], name, rate)
        if name not in dated:
            rows.append(row)
            continue
        # A published price change keeps the rate card's dated periods: a speed
        # multiple (OpenAI's Ultrafast promotion) or the list price itself (Google's
        # introductory Gemini prices). LiteLLM must agree with the period that
        # covers the release date; the other periods take the rate card's prices.
        # A rate card that names no speed for the model (Google's Priority is a
        # service tier, not a speed) keeps LiteLLM's.
        card_speeds = any(speed in (period.get("tier_multipliers") or {}) for period in dated[name] for speed in SPEEDS)
        for period in dated[name]:
            speeds = row["tier_multipliers"]
            if card_speeds:
                speeds = {s: m for s, m in (period.get("tier_multipliers") or {}).items() if s in SPEEDS}
            current = (period.get("effective_from") or "") <= day < (period.get("effective_until") or "9999")
            if current and speeds != row["tier_multipliers"]:
                raise ValueError(f"{name}: LiteLLM speed rates differ from the rate card's current period")
            priced = {**row, "effective_from": period.get("effective_from"),
                      "effective_until": period.get("effective_until"), "tier_multipliers": speeds}
            listed = [period[key] for key in LIST_PRICES]
            if listed != [row[key] for key in LIST_PRICES]:
                if current:
                    raise ValueError(f"{name}: LiteLLM prices differ from the rate card's current period")
                if "threshold_input_tokens" in row or "threshold_input_tokens" in period:
                    raise ValueError(f"{name}: a dated long-prompt price requires a script update")
                # The other cache rates keep their ratio to input, as T3 does for a missing one.
                ratio = Fraction(period["input_nanos_per_token"], row["input_nanos_per_token"])
                for key in ("cache_write_nanos_per_token", "cache_write_1h_nanos_per_token"):
                    if priced.get(key) is not None:
                        value = priced[key] * ratio
                        if value.denominator != 1:
                            raise ValueError(f"{name}: unsupported sub-nanodollar rate")
                        priced[key] = int(value)
                priced.update(zip(LIST_PRICES, listed))
                priced["source_url"] = period["source_url"]
            rows.append(priced)
    present = {name for row in rows for name in row["models"]}
    # Preserve existing published model support when LiteLLM has no matching ID.
    # These rows retain their official source URL, making the fallback auditable,
    # and the published speed multipliers the T3 calculation uses.
    # Dated rows (a promotional speed price) keep their periods. OpenAI's default
    # cache write lifetime is 30 minutes, Anthropic's 5 minutes.
    for row in provider["rates"]:
        missing = [name for name in row["models"] if name not in present]
        tiers = row.get("tier_multipliers") or {}
        write = row.get("cache_write_5m_nanos_per_token")
        if write is None:
            write = row.get("cache_write_30m_nanos_per_token")
        if missing:
            rows.append({"provider": row["provider"], "models": missing, "effective_from": row.get("effective_from"),
                         "effective_until": row.get("effective_until"), "source_url": row["source_url"],
                         "input_nanos_per_token": row["input_nanos_per_token"],
                         "cache_read_nanos_per_token": row.get("cache_read_nanos_per_token"),
                         "cache_write_nanos_per_token": write,
                         "cache_write_5m_nanos_per_token": None, "cache_write_1h_nanos_per_token": None,
                         "output_nanos_per_token": row["output_nanos_per_token"],
                         "tier_multipliers": {speed: tiers[speed] for speed in SPEEDS if speed in tiers},
                         # A long-prompt price; omitted cache rates keep their ratio to input.
                         **{key: row[key] for key in LONG_PROMPT if row.get(key) is not None}})
    groups = {}
    for row in rows:
        key = json.dumps({k: v for k, v in row.items() if k != "models"}, sort_keys=True)
        groups.setdefault(key, {**row, "models": []})["models"].extend(row["models"])
    rates = list(groups.values())
    for row in rates:
        row["models"].sort()
    rates.sort(key=lambda row: (row["provider"], row["models"][0], row["effective_from"] or ""))
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
