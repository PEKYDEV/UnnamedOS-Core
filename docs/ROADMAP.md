# Public roadmap

The next public-core work is deliberately incremental:

1. integrate inactive UEFI table-frame allocation and bounded hierarchy materialization with final-map reservations, without switching CR3;
2. add runtime CPU probing and independently verify the reviewed transition before any address-space activation;
3. define hardware discovery and interrupt contracts before implementing general drivers;
4. publish additional subsystems only after their architecture, security boundary, tests, and licensing are reviewed.

This roadmap is directional, not a release commitment. Production security claims require independent expert audit.
