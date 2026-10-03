#!/usr/bin/env python3
"""Opt-in public documentation smoke tests. Python is only needed for this audit.

Fetch once per case, then exercise all link modes against identical saved bytes.
Results are local snapshots, not stable fixtures or a guarantee of completeness.
"""
import argparse
import concurrent.futures
import datetime
import json
from pathlib import Path
import shutil
import subprocess


def run(command):
    return subprocess.run(command, capture_output=True, text=True, timeout=65)


def check(case, args):
    folder = args.out / case["name"]
    folder.mkdir(parents=True, exist_ok=True)
    raw = folder / "source.html"
    result = dict(case)
    command = [str(args.binary), case["url"], "--timeout", "40",
               "--user-agent", "urlmd-site-audit (+https://github.com/mocilukalbj/urlmd)",
               "--save-source", str(raw), "--no-metadata", "-o", str(folder / "keep.md")]
    if case["mode"] == "html":
        command.append("--html")
    try:
        if not args.cached:
            fetched = run(command)
            if fetched.returncode:
                result.update(status="fetch_or_conversion_failed", error=fetched.stderr.strip())
                return result
        meta = json.loads(Path(str(raw) + ".meta.json").read_text())
        result.update(content_type=meta["content_type"], final_url=meta["final_url"],
                      download_selection=meta["selection"], source_sha256=meta["source_sha256"])
        source = raw
        if meta["format"] in ("native-markdown", "plain-text"):
            source = folder / "source.md"
            shutil.copyfile(raw, source)
        result["characters"] = {}
        for mode in ("keep", "relative", "text"):
            output = folder / (mode + ".md")
            converted = run([str(args.binary), "--input", str(source), "--base-url", meta["final_url"],
                             "--encoding", meta["encoding"], "--links", mode, "--no-metadata", "-o", str(output)])
            if converted.returncode:
                raise RuntimeError(converted.stderr.strip())
            result["characters"][mode] = len(output.read_text())
        # Text mode avoids false failures when expected phrases cross a Markdown link.
        content = (folder / "text.md").read_text()
        missing = [phrase for phrase in case["expected"] if phrase not in content]
        result.update(status="keyword_failed" if missing else "passed", missing=missing)
        if args.baseline:
            baseline = run([str(args.baseline), "--input", str(source), "--base-url", meta["final_url"],
                            "--encoding", meta["encoding"], "--no-metadata", "-o", str(folder / "baseline.md")])
            if baseline.returncode:
                raise RuntimeError("Baseline: " + baseline.stderr.strip())
    except Exception as error:
        result.update(status="audit_failed", error=str(error))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--out", type=Path, required=True, help="Directory for local source snapshots and report")
    parser.add_argument("--cached", action="store_true", help="Reuse saved responses; do not contact websites")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    if args.baseline:
        args.baseline = args.baseline.resolve()
    args.out = args.out.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    cases = json.loads(Path(__file__).with_name("sites.json").read_text())
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        results = list(pool.map(lambda case: check(case, args), cases))
    report = {"checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat(), "cases": results}
    (args.out / "report.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    for result in results:
        print(result["name"], result["status"], result.get("error", result.get("missing", "")))
    raise SystemExit(int(any(result["status"] != "passed" for result in results)))


if __name__ == "__main__":
    main()
