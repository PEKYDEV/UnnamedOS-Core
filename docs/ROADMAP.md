# Public roadmap

The next public-core work is deliberately incremental:

1. implement a fixed-capacity initial page-table planner/owner and versioned ownership metadata without switching CR3;
2. integrate the reviewed transition only after allocation rollback, CPU preconditions, final-map reservations, and low-map removal are independently testable;
3. define hardware discovery and interrupt contracts before implementing general drivers;
4. publish additional subsystems only after their architecture, security boundary, tests, and licensing are reviewed.

This roadmap is directional, not a release commitment. Production security claims require independent expert audit.
