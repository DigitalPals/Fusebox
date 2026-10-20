#!/usr/bin/env python3
"""Aggregate native observation JSONL into cache-gap and candidate unused-write statistics.
Input is a private local metadata export; output contains no session/account/client ids.
"""
import argparse
import collections
import json
import statistics
import sys


def analyze(observations, ttl_seconds=300, as_of_ms=None):
    groups = collections.defaultdict(list)
    latest = 0
    for item in observations:
        if item.get("source") != "proxy":
            continue
        timestamp = item.get("event_at_ms")
        if type(timestamp) is not int:
            continue
        latest = max(latest, timestamp)
        session = item.get("session_id")
        if not session:
            continue
        groups[(item.get("provider"), item.get("account_id"), item.get("client_id"),
                item.get("actual_model") or item.get("requested_model"), session)].append(item)
    end = latest if as_of_ms is None else as_of_ms
    counts = collections.Counter()
    gaps = []
    for records in groups.values():
        records.sort(key=lambda r: r["event_at_ms"])
        times = [row["event_at_ms"] for row in records]
        gaps.extend((b-a)/1000 for a, b in zip(times, times[1:]))
        read_times = [row["event_at_ms"] for row in records if (row.get("tokens", {}).get("cache_read") or 0) > 0]
        import bisect
        for row in records:
            write = row.get("tokens", {}).get("cache_write")
            if type(write) is not int or write <= 0:
                continue
            counts["write_observations"] += 1
            expiry = row["event_at_ms"] + ttl_seconds * 1000
            if expiry > end:
                counts["right_censored_writes"] += 1
                continue
            counts["eligible_writes"] += 1
            pos = bisect.bisect_right(read_times, row["event_at_ms"])
            if pos == len(read_times) or read_times[pos] > expiry:
                counts["candidate_unused_writes"] += 1
                counts["candidate_unused_write_tokens"] += write
    return {"schema_version": 1, "ttl_seconds": ttl_seconds, "as_of_ms": end,
            "counts": dict(counts), "request_gaps_seconds": {
                "observations": len(gaps), "median": statistics.median(gaps) if gaps else None,
                "over_5m": sum(gap > 300 for gap in gaps), "over_1h": sum(gap > 3600 for gap in gaps)},
            "limitations": ["cohort-level candidates; exact prefix reuse is unproven",
                            "external provider traffic and missing observations can reuse writes",
                            "TTL measured from request start; choose the provider's actual TTL",
                            "list-price token categories do not establish subscription quota costs"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("observations", help="Native observation JSONL; '-' for stdin")
    parser.add_argument("--ttl-seconds", type=int, choices=(300, 1800, 3600), default=300)
    parser.add_argument("--as-of-ms", type=int)
    args = parser.parse_args()
    try:
        source = sys.stdin if args.observations == "-" else open(args.observations)
        with source:
            observations = [json.loads(line) for line in source if line.strip()]
        print(json.dumps(analyze(observations, args.ttl_seconds, args.as_of_ms), indent=2))
    except (OSError, ValueError, TypeError, AttributeError):
        print('{"error":"invalid_observation_export"}', file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
