#!/usr/bin/env python3
"""Heuristic Rust audit; findings require human review, not automatic fixes.

Usage: python3 scripts/audit_rust.py [path] [--json] [--fail-on-findings]
Run built-in detector checks with --self-test. No third-party packages needed.
This is a text scanner, not a Rust parser or a replacement for tests/Clippy.
Use --include-size-casts for noisy cast checks: this scanner cannot infer types.
It excludes conventional trailing test modules unless --include-tests is set.
"""
import argparse
import json
import re
import sys
from pathlib import Path

SKIP = {"target", "vendor", ".git", "node_modules", ".codex", ".agents"}


def scan(text, include_tests=False, include_size_casts=False):
    if not include_tests:
        text = re.split(r"(?m)^#\[cfg\((?:all\()?test\b", text, maxsplit=1)[0]
    findings = []

    def emit(rule, offset, message):
        finding = {"rule": rule, "line": text.count("\n", 0, offset) + 1,
                   "message": message}
        if finding not in findings:
            findings.append(finding)

    for match in re.finditer(r"\b(?:std::fs::)?hard_link\s*\(", text):
        nearby = text[max(0, match.start() - 200):match.end() + 700]
        if not re.search(r"copy|fallback|Unsupported", nearby):
            emit("hard-link-only", match.start(),
                 "Hard-link publishing may fail on FAT/exFAT; check for an exclusive-copy fallback.")
    for match in re.finditer(r"\bread_dir\s*\(", text):
        nearby = re.split(r"(?m)^\s*(?:pub\s+)?fn\s+", text[match.end():match.end() + 1600], maxsplit=1)[0]
        if re.search(r"read_to_string\s*\(\s*&?\w+\s*\)", nearby) and not re.search(
                r"is_file\s*\(|file_type\s*\(|metadata\s*\(|extension\s*\(", nearby):
            emit("directory-as-file", match.start(),
                 "Directory entries reach a text read without a visible file-type filter.")
    for match in re.finditer(
            r"\(\s*(?:blocks|count|index_len|length|len|size)\s*\+\s*\d+\s*\)\s*\*\s*\d+", text):
        emit("unchecked-size-arithmetic", match.start(),
             "Size arithmetic can overflow before validation; inspect input provenance and use checked operations.")
    if include_size_casts:
        for match in re.finditer(r"\bas\s+usize\b", text):
            emit("size-cast", match.start(),
                 "Check source type and bounds: u32 to usize is lossless on 32/64-bit targets; wider integers may truncate.")
    for match in re.finditer(r"\blet\s+_\s*=\s*[^;\n]*(?:write_all|flush|finish|persist)\s*\(", text):
        emit("discarded-write-error", match.start(),
             "A write/finalization error is discarded; verify that best-effort behavior is intentional.")
    return findings


def rust_files(root):
    if root.is_file():
        if root.suffix == ".rs":
            yield root
        return
    for child in sorted(root.iterdir()):
        if child.is_symlink() or child.name in SKIP or child.name.startswith("."):
            continue
        if child.is_dir():
            yield from rust_files(child)
        elif child.suffix == ".rs":
            yield child


def self_test():
    import unittest

    class Detectors(unittest.TestCase):
        def test_risky_patterns(self):
            cases = {
                "hard-link-only": "std::fs::hard_link(source, target)",
                "directory-as-file": "read_dir(dir); read_to_string(&path);",
                "unchecked-size-arithmetic": "if (blocks + 1) * 4 > file_len {}",
                "size-cast": "let n = size as usize;",
                "discarded-write-error": "let _ = enc.write_all(block);",
            }
            for rule, source in cases.items():
                with self.subTest(rule=rule):
                    self.assertIn(rule, [f["rule"] for f in scan(source, include_size_casts=True)])

        def test_guards_and_test_exclusion(self):
            self.assertEqual(scan("hard_link(a,b); copy_output_exclusive(a,b);"), [])
            self.assertEqual(scan("read_dir(dir); if path.is_file() { read_to_string(path); }"), [])
            self.assertEqual(scan("let n = count.checked_add(1)?.checked_mul(4)?;"), [])
            self.assertEqual(scan('#[cfg(test)]\nmod tests { let n = size as usize; }'), [])
            self.assertEqual(scan("// heading\nlet n = size as usize;", include_size_casts=True)[0]["line"], 2)

        def test_casts_are_opt_in_and_deduplicated(self):
            source = "let size = [w as usize, h as usize];"
            self.assertEqual(scan(source), [])
            self.assertEqual(len(scan(source, include_size_casts=True)), 1)

        def test_nested_sources(self):
            import tempfile
            with tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                (root / "src/nested").mkdir(parents=True)
                (root / "src/nested/mod.rs").write_text("")
                (root / "src/notes.txt").write_bytes(b"\xff")
                (root / "vendor").mkdir()
                (root / "vendor/ignored.rs").write_text("")
                self.assertEqual(list(rust_files(root)), [root / "src/nested/mod.rs"])

    result = unittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromTestCase(Detectors))
    return 0 if result.wasSuccessful() else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", nargs="?", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--json", action="store_true", help="Emit machine-readable candidates")
    parser.add_argument("--include-tests", action="store_true")
    parser.add_argument("--include-size-casts", action="store_true", help="Include noisy casts requiring manual type/bounds review")
    parser.add_argument("--fail-on-findings", action="store_true", help="Exit 1 when candidates exist")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    findings = []
    try:
        for path in rust_files(args.path):
            for finding in scan(path.read_text(encoding="utf-8"), args.include_tests, args.include_size_casts):
                findings.append({"path": str(path), **finding})
    except (OSError, UnicodeError) as error:
        print(f"Audit failed: {error}", file=sys.stderr)
        return 2
    if args.json:
        print(json.dumps(findings, indent=2))
    else:
        for item in findings:
            print(f'{item["path"]}:{item["line"]}: [{item["rule"]}] {item["message"]}')
        print(f"{len(findings)} review candidates; heuristic matches can be false positives.")
    return int(args.fail_on_findings and bool(findings))


if __name__ == "__main__":
    sys.exit(main())
