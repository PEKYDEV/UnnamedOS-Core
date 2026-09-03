# Public roadmap

The next public-core work is deliberately incremental:

1. finalize the higher-half ELF layout and complete mapping inputs before any address-space activation;
2. emit reviewed BootInfo v2 ownership metadata, then implement the ordered NXE/WP/CR3 transition as a separate state-mutating package;
3. define hardware discovery and interrupt contracts before implementing general drivers;
4. publish additional subsystems only after their architecture, security boundary, tests, and licensing are reviewed.

This roadmap is directional, not a release commitment. Production security claims require independent expert audit.
