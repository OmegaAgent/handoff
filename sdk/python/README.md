# handoff-human

The Python client for [Handoff](https://github.com/OmegaAgent/handoff). One file, standard library
only, no dependencies — copy it into your project or install it.

```bash
pip install handoff-human
```

```python
import handoff

handoff.configure(base_url="https://handoff.omegas.dev")   # or set HANDOFF_URL

# Ask a person a question. Blocks until they answer; returns their text.
address = handoff.ask("Which shipping address should I use?", timeout_s=600)

# Or hand over a browser your agent cannot get past. Blocks until they say it is clear.
handoff.clear_wall(
    reason="A human-verification checkbox is blocking checkout",
    live_view_url=browser.live_url,
    resume_url=browser.resume_url,
)
```

`ask()` raises `HandoffTimeout` if nobody answers before `timeout_s`, unless you pass `default=`.

## Licence

MIT — see `LICENSE` in this directory. The rest of the Handoff repository is Apache-2.0; the SDKs are
MIT so they vendor cleanly into anything.

## Module rename in 0.2.0

The module is now `handoff`. The old `human` module still imports and forwards to it with a
`DeprecationWarning`, and is **removed in 0.3.0**:

```python
import human      # works in 0.2.x, warns, gone in 0.3.0
import handoff    # do this
```

The distribution name on PyPI is unchanged: `handoff-human`.
