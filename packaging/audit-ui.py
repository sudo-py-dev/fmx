#!/usr/bin/env python3
"""Fast, read-only FMX UI audit. Lexes Rust and balances calls; no dependencies.
Findings are review candidates, not proof of GNOME compliance or accessibility.
Usage: python3 packaging/audit-ui.py [repo] [--json report.json]
"""
import argparse
import bisect
import collections
import json
from pathlib import Path
import re
import time


RAW = re.compile(r'(?:br|r)(#*)"')
CHAR = re.compile(r"'(?:\\.|[^'\\\n])'")
WORD = re.compile(r'[A-Za-z_][A-Za-z_0-9]*|[0-9]+(?:\.[0-9]+)?')


def lex(source):
    tokens = []
    i = 0
    while i < len(source):
        if source[i].isspace():
            i += 1
            continue
        start = i
        if source.startswith('//', i):
            end = source.find('\n', i)
            i = len(source) if end < 0 else end
            continue
        if source.startswith('/*', i):
            depth = 1
            i += 2
            while i < len(source) and depth:
                if source.startswith('/*', i):
                    depth += 1
                    i += 2
                elif source.startswith('*/', i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
            continue
        raw = RAW.match(source, i)
        if raw:
            closing = '"' + raw[1]
            end = source.find(closing, i + len(raw[0]))
            i = len(source) if end < 0 else end + len(closing)
        elif source[i] == '"':
            i += 1
            while i < len(source):
                if source[i] == '\\':
                    i += 2
                elif source[i] == '"':
                    i += 1
                    break
                else:
                    i += 1
        else:
            char = CHAR.match(source, i)
            word = WORD.match(source, i)
            i += len(char[0]) if char else len(word[0]) if word else 1
        tokens.append((source[start:i], start, i))
    return tokens


def balanced(tokens):
    stack, pairs = [], {}
    for i, (value, _, _) in enumerate(tokens):
        if value in ('(', '[', '{'):
            stack.append(i)
        elif value in (')', ']', '}') and stack:
            opening = stack.pop()
            if tokens[opening][0] == {')': '(', ']': '[', '}': '{'}[value]:
                pairs[opening] = i
    return pairs


def args(tokens, opening, pairs):
    result, start, i = [], opening + 1, opening + 1
    end = pairs[opening]
    while i < end:
        if i in pairs:
            i = pairs[i] + 1
        elif tokens[i][0] == ',':
            result.append((start, i))
            start = i = i + 1
        else:
            i += 1
    if start < end:
        result.append((start, end))
    return result


def audit(root):
    findings, inventory, referenced = [], {}, set()
    def add(rule, path, line, note, severity='review'):
        findings.append(dict(rule=rule, file=str(path.relative_to(root)), line=line,
                             severity=severity, message=note))
    files = sorted((root / 'src').glob('*.rs'))
    for path in files:
        source = path.read_text()
        tokens = lex(source)
        pairs = balanced(tokens)
        vals = [t[0] for t in tokens]
        newlines = [i for i, c in enumerate(source) if c == '\n']
        line = lambda index: bisect.bisect_left(newlines, tokens[index][1]) + 1
        for i, value in enumerate(vals[:-1]):
            if vals[i + 1] != '(' or i + 1 not in pairs:
                continue
            end = pairs[i + 1]
            arguments = args(tokens, i + 1, pairs)
            if value in ('tr', 'trf') and arguments:
                key = vals[arguments[0][0]]
                if key.startswith('"') and key.endswith('"'):
                    referenced.add(key[1:-1])
            if value == 'collapsing':
                add('rtl-disclosure', path, line(i), 'Native collapsing header: verify RTL placement; use the shared direction-aware expander.')
            if value == 'checkbox' and i and vals[i-1] == '.' and path.name != 'dir.rs':
                add('rtl-checkbox', path, line(i), 'Use dir::checkbox for mirrored box and label placement.')
            if value == 'left_to_right' and path.name not in ('dir.rs', 'preview.rs'):
                add('fixed-direction', path, line(i), 'Review fixed LTR layout; directional transport and raw diagnostics may be intentional.')
            if value == 'set_min_size':
                add('minimum-size', path, line(i), 'Check minimum size against small windows and zoom; static scan cannot establish clipping.')
            if value == 'new' and i >= 3 and vals[i-3:i] == ['Window', ':', ':']:
                j = end + 1
                methods = []
                while j + 2 < len(vals) and vals[j] == '.' and vals[j+2] == '(' and j+2 in pairs:
                    methods.append(vals[j+1])
                    j = pairs[j+2] + 1
                if 'anchor' not in methods:
                    add('dialog-position', path, line(i), 'Window has no center anchor; review whether it should be a modal.')
            if value == 'button' and i >= 3 and vals[i-3:i] == ['widgets', ':', ':'] and len(arguments) == 5:
                text_start, text_end = arguments[2]
                if vals[text_start:text_end] == ['""'] and 'widget_info' not in source[tokens[end][2]:tokens[end][2]+240]:
                    add('icon-action-name', path, line(i), 'Icon-only button: verify explicit widget_info action label; tooltip alone is insufficient.')
        if path.name == 'catalog.rs':
            try:
                start = vals.index('Tool', next(i for i in range(len(vals)-2) if vals[i:i+3] == ['pub','enum','Tool']))
                opening = vals.index('{', start)
                variants = args(tokens, opening, pairs)
                inventory['tool_variants'] = [vals[a] for a, b in variants if a < b]
            except (ValueError, StopIteration):
                inventory['tool_variants'] = []
    locales = {}
    duplicates = []
    for path in sorted((root / 'locales').glob('*.json')):
        def pairs_hook(items):
            result = {}
            for key, value in items:
                if key in result:
                    duplicates.append((path, key))
                result[key] = value
            return result
        locales[path.stem] = json.loads(path.read_text(), object_pairs_hook=pairs_hook)
    english = locales['en']
    placeholders = lambda text: collections.Counter(re.findall(r'\{([A-Za-z_][A-Za-z_0-9]*)\}', text))
    for lang, strings in locales.items():
        path = root / 'locales' / (lang + '.json')
        for key in english.keys() - strings.keys():
            add('missing-translation', path, 1, key, 'error')
        for key in strings.keys() - english.keys():
            add('extra-translation-key', path, 1, key)
        for key in strings.keys() & english.keys():
            if placeholders(strings[key]) != placeholders(english[key]):
                add('translation-placeholders', path, 1, key, 'error')
            if not strings[key].strip():
                add('empty-translation', path, 1, key, 'error')
    for path, key in duplicates:
        add('duplicate-translation', path, 1, key, 'error')
    for key in sorted(referenced - english.keys()):
        add('unknown-ui-string', root / 'locales/en.json', 1, key, 'error')
    inventory.update(rust_files=len(files), languages=sorted(locales), literal_translation_keys=len(referenced))
    return dict(inventory=inventory, counts=dict(collections.Counter(f['rule'] for f in findings)), findings=findings,
                limitations='Heuristic static review only. Cannot verify rendered layout, contrast, keyboard navigation, dynamic translation keys, or full tool behavior.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', nargs='?', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--json', type=Path)
    options = parser.parse_args()
    started = time.perf_counter()
    report = audit(options.root.resolve())
    report['elapsed_seconds'] = round(time.perf_counter() - started, 3)
    if options.json:
        options.json.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({k: v for k, v in report.items() if k != 'findings'}, ensure_ascii=False, indent=2))
    for item in report['findings']:
        print(f"{item['file']}:{item['line']} [{item['rule']}] {item['message']}")
    return int(any(f['severity'] == 'error' for f in report['findings']))


if __name__ == '__main__':
    raise SystemExit(main())
