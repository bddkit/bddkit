# Macros

A file under `macro_paths` is a YAML list, one domain step per entry:

```yaml
- step: 'I log in as "{user}" with password "{password:secret}"'
  exports: [login_secret]
  do:
    - the request body is: |
        {"user": "<<user>>", "password": "<<password>>"}
    - I request "/login" using HTTP POST
    - the response code is 200
    - extract "token" from JSON as "login_secret"
```

- `step:` is a Cucumber Expression. `{name}` matches any text; `{n:uint}`, `int`, `float`, `word`, `method` restrict it; `{name:secret}` also masks the value. Escape a literal `(`, `)`, `/`, `{` with `\`.
- The body reads a parameter as `<<name>>`.
- A step that takes a doc string is a one-key map: its text is the key.
- `exports` lists the variables the caller can read afterwards; nothing else survives.
- Macros may call macros (depth 16). `bddkit doctor` reports conflicts and cycles.
