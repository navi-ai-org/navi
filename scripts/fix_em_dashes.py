EM = "\u2014"
path = 'crates/navi-tui/src/app.rs'
with open(path, 'r') as f:
    s = f.read()

# Replace literal `{em}` (which snuck in because I forgot to use f-string)
# with the proper Rust string containing the em-dash character.
replacements = [
    ('format!("{em} Recent {em}"),', f'format!("{EM} Recent {EM}"),'),
    ('format!("{em} Connected {em}"),', f'format!("{EM} Connected {EM}"),'),
    ('format!("{em} Other providers {em}"),', f'format!("{EM} Other providers {EM}"),'),
]
for old, new in replacements:
    count = s.count(old)
    print(f'"{old}" -> {count} occurrences')
    s = s.replace(old, new)

with open(path, 'w') as f:
    f.write(s)
print('done')
