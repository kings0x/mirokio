Integration tests for `mirokio` should live in this directory.

We are keeping edge-case and correctness tests next to the modules they verify,
and reserving `tests/` for multi-module behavior such as spawn + timer + worker
interactions.
