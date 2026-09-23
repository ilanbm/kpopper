#!/usr/bin/env python3.14
"""Regenerate the pinned Python text fixtures used by native/src/python_text.rs.

usage: generate_python_text.py OUTPUT_JSON

Writes the textwrap corpus to OUTPUT_JSON and prints, on stdout, the ALNUM and
DECIMAL tables and the per-character digest the module's tests pin.
"""
import hashlib
import json
import random
import sys
import textwrap
import unicodedata

if sys.version_info[:2] != (3, 14) or unicodedata.unidata_version != '16.0.0':
    raise SystemExit('needs Python 3.14 (Unicode 16.0.0)')
if len(sys.argv) != 2:
    raise SystemExit(__doc__)


def ranges(predicate):
    out, start = [], None
    for code in range(0x110000):
        held = predicate(chr(code))
        if held and start is None:
            start = code
        if not held and start is not None:
            out.append((start, code - 1))
            start = None
    if start is not None:
        out.append((start, 0x10FFFF))
    return out


def table(name, rows):
    body = ''.join(f'    (0x{a:x}, 0x{b:x}),\n' for a, b in rows)
    return f'const {name}: &[(u32, u32)] = &[\n{body}];\n'


digest = hashlib.sha256()
for code in range(0x110000):
    if 0xD800 <= code <= 0xDFFF:
        digest.update(bytes(3))
        continue
    c = chr(code)
    digest.update(bytes([c.isalnum(), c.isdecimal(), c.isspace()]))
print(table('ALNUM', ranges(str.isalnum)) + table('DECIMAL', ranges(str.isdecimal)), end='')
print('// digest', digest.hexdigest())

# Pieces chosen for the wrapper's rules: hyphens inside and between words,
# em-dashes, digits beside hyphens, combining marks, non-ASCII spaces that
# the splitter keeps inside words, control whitespace it turns into spaces,
# and words longer than a line.
PIECES = (['a', 'b', 'Z', 'é', 'ש', 'ל', '日', '1', '0', '٣', '²', 'Ⅻ', '_'] * 3
          + ['-'] * 8 + ['--', '---', ' '] * 4
          + ['  ', '\t', '\n', '\r\n', '\x0b', '\x0c', '.', ',', '!', '?', "'", '"', '&',
             ';', ':', '/', '(', ')', '…', '#', 'ְ', '́', ' ', ' ',
             '　', '\x1c', 'well-known', 'dark-matter', 'e-mail', 'a-b-c', 'x-1',
             '1-x', 'ab-cd-ef', 'baryon-acceleration', 'long' * 6, 'hyphen-' * 5, '-' * 12])
WIDTHS = [1, 2, 3, 5, 8, 10, 12, 20, 36, 36, 36, 36, 50, 70, 96]
FIXED = [
    '', ' ', '   ', 'a', 'Under the stated models, the selected evidence supports a dark-matter '
    'account across scales.', 'Require a galaxy-scale explanation to address the tight '
    'baryon-acceleration relation.', 'one tab, for the night the greenhouse is decided on',
    'Look, goof-ball -- use the -b option!', 'Hello there -- you goof-ball, use the -b option!',
    'a--b', '--a', 'a--', 'a---b c', 'ab-cd', 'a-b', 'ab-c', 'a-bc', 'ab-1c', '1a-bc', 'x_y-z_w',
    'aa-bb-cc-dd-ee-ff-gg-hh-ii-jj-kk-ll-mm-nn', '-' * 50, 'a' * 40, 'a' * 35 + ' ' + 'b' * 40,
    'word ' * 20, '  leading and trailing  ', 'tab\there\tand\tthere', 'שלום-עולם גדול-מאוד',
]

rng = random.Random(20260923)
cases = [{'text': text, 'width': 36} for text in FIXED]
cases += [{'text': text, 'width': width} for text in FIXED[4:6] for width in (10, 20)]
for _ in range(600):
    text = ''.join(rng.choice(PIECES) for _ in range(rng.randint(1, 40)))
    cases.append({'text': text, 'width': rng.choice(WIDTHS)})
for case in cases:
    case['lines'] = textwrap.wrap(case['text'], case['width'])
with open(sys.argv[1], 'w', encoding='ascii') as out:
    json.dump({'python': sys.version.split()[0], 'cases': cases}, out, ensure_ascii=True,
              separators=(',', ':'))
    out.write('\n')
