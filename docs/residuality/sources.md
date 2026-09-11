# Quellen, Geltung und bekannte Lücken

Stand: 2026-09-08, Projekt-HEAD `e2b5d25` plus bereits vor diesem Auftrag bestehende Änderungen. `factory-recovery`, ADR 0020 und Teile von Store/Task/Session sind Arbeitsstand; ihre Präsenz bedeutet keine abgeschlossene Produktlieferung. Diese Analyse hat diese Dateien nicht verändert und keine Live-Incidents beobachtet.

## Architekturinputs

- [Company-Design](../../../../.specs/design.md): v1-Primitives, lokale Plattform, Schreiballowlist, Delivery und kooperative Trustgrenze.
- [AGENTS.md](../../AGENTS.md): geltende Projektgrenzen. Die stärkere aktuelle `.factory/`-Schreiballowlist ist bei Konflikten maßgeblich.
- [Backlog](../implementation-backlog.md): Lieferplan, insbesondere Slices 5–13. Enthält teilweise veraltete offene Punkte; nicht als alleiniger Implementierungsstatus verwendet.
- [ADR 0001](../adr/0001-microkernel-workers-and-scoped-plugins.md): Microkernel, Lifetimes, Process-Executor und scoped plugins; Threads/Messages durch ADR 0010 für v1 abgegrenzt.
- [ADR 0002](../adr/0002-daemon-and-api-transport-plugins.md): ein Daemon, Transportplugins, Bootstrap, Authentisierung; älterer Socketpfad wird als Konflikt benannt.
- [ADR 0003](../adr/0003-logical-api-and-event-sourcing-v1.md): Eventkanon, Replay, Idempotenz, sichere Referenzen, Saga-/Kompensationsvertrag.
- [ADR 0007](../adr/0007-supervised-plugin-process-protocol-v1.md): überwachte Aktivierungen, Framing, Capabilitybindung, Grenzen der Prozessisolation.
- [ADR 0010](../adr/0010-design-baseline-reconciliation.md): v1 versus Zielzustand.
- [ADR 0012](../adr/0012-slice-2-sqlite-driver-migrations-and-locking.md): SQLite, Konsistenz und Leasezustände.
- [ADR 0013](../adr/0013-slice-4-context-compilation-policy.md): Kontextfehler statt stiller Auslassung, keine unbounded Linktraversierung.
- [ADR 0014](../adr/0014-daemon-owns-all-mutations.md): Mutationseigentum und benannte Offline-Doctor-Ausnahme.
- [ADR 0016](../adr/0016-slice-3-registry-projection-and-reconcile.md): aktuelle Registry versus Filesystembeobachtung.
- [ADR 0017](../adr/0017-slice-5-pi-adapter-observation-source.md): Pi-Korrelation und Beobachtungsautorität.
- [ADR 0018](../adr/0018-updating-an-installed-instance.md), [ADR 0019](../adr/0019-restore-and-what-a-recovered-database-may-claim.md): Upgrade-/Restoregrenzen und Backupumfang.
- [ADR 0020](../adr/0020-run-telemetry-and-the-evaluation-bench.md): Instrumentvergleich, pinned tuple, menschliche Produktionsverifikation und Fixture-Gates; Benchrunner bleibt post-v1.
- [Task-Migrationsplan](../task-store-migration-plan.md): produktiver Prototyp und dokumentierter Schema-Namenskonflikt. Keine neue Live-DB-Untersuchung in diesem Auftrag.

## Konkrete statische Implementierungsbefunde

| Fundstelle | Befund | Verwendet für |
|---|---|---|
| [reconnect.rs](../../crates/factory-recovery/src/reconnect.rs), `identifies_same_session` | Missing-ID-Zweig liefert `true`; vorhandene Tests/Kommentare stützen diesen Fallback bewusst. | S032/S033/S157, C01, T01 |
| Dieselbe Datei, `reconnect_after_supervisor_restart` | Nicht zur Promotion akzeptiertes `Ok` führt zu `give_up...` und damit Leasefreigabe. | S031/S034/S040, A1-D1 |
| [evidence.rs](../../crates/factory-recovery/src/evidence.rs) | Autorität allein ist getrennt von Identität/Lebendigkeit; Consumer müssen zusätzliche Evidenz prüfen. | R01-Vertrag |
| [factory-adapter](../../crates/factory-adapter/src/lib.rs), `HerdrCli::run` | `Command::output()` ohne eigene Deadline; geplante Hostgrenzen sind noch nicht durch diesen Aufruf geliefert. | S041–S047, C12 |
| [Store-Schema](../../crates/factory-store/src/schema.rs) | Kommentar benennt fehlenden Event Store; mutable Tabellen sind nicht kanonisch replaybare Projektionen. | S018/S019/S158, A0/A3 |
| [deliver.rs](../../crates/factory-task/src/deliver.rs) | Intent vor Writer; `authorise_resume` erhöht persistenten Counter, hat aber selbst keine vollständige Actor-/Payload-Freigabeevidenz. | S071/S074/S136, C03/C04 |
| [assign.rs](../../crates/factory-task/src/assign.rs) | Transaktionale Auswahl und nonterminal-gebundene Belegung statt bloßem `running`-Index. | S073/S077; vorhandenen Schutz nicht als Lücke darstellen |

Die 200 Karten sind **hypothetische Angriffe auf Annahmen**, keine Behauptungen über 200 existierende Bugs. Auch die in der ersten Analyse beschriebenen F01–F05 sind statische Reviewbefunde, keine Produktionsvorfälle. Vor Patches die dann aktuellen Dateien erneut lesen.

## Methodischer Quellenumfang

Das Verfahren ist durch die Residuality-Idee motiviert und für dieses Projekt operationalisiert. Keine externen Webseiten, Bücher oder unabhängigen Fachinterviews wurden in diesem Auftrag konsultiert. Es werden keine zufällige Stichprobe, kritische Wahrscheinlichkeitsschwelle oder mathematische Konvergenz von Residues behauptet. Die methodische Eigenleistung und ihre Biasgrenzen sind in [methodology.md](methodology.md) offengelegt.
