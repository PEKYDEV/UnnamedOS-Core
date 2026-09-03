# Public roadmap

The next public-core work is deliberately incremental:

1. add production CPU capability probing and activation-readiness validation without switching CR3;
2. complete and independently verify the reviewed higher-half transition before any address-space activation;
3. define hardware discovery and interrupt contracts before implementing general drivers;
4. publish additional subsystems only after their architecture, security boundary, tests, and licensing are reviewed.

This roadmap is directional, not a release commitment. Production security claims require independent expert audit.
