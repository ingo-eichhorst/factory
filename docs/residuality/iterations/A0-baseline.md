# A0 — Ausgangsarchitektur und zu prüfende Zusagen

Stand 2026-09-08, `e2b5d25` plus bestehender Arbeitsstand. A0 bezeichnet den **dokumentierten Architekturvertrag**, nicht ein lauffähiges Gesamtsystem. Der produktive Python-Prototyp, Rust-Bibliotheken und Zielarchitektur sind drei verschiedene Ebenen. Die Befunde F01–F05 der [ersten Analyse](../../residuality-stressor-analysis.md) wurden durch gezielte Fundstellensuche erneut auf Aktualität geprüft.

## Bestehende Form

```text
CLI / Agent-Tool / spätere weitere Clients
  → API-Transport-Plugin mit Authentisierung
  → ein Daemon: Autorisierung + Task-/Session-/Lease-Invarianten
       → kanonische Events / Projektionen / Idempotenz / Outbox [Vertrag]
       → überwachte Scope-Plugins [Vertrag]
            → Herdr / Harness / externe Teilnehmer
```

Behalten: fünf v1-Primitives, Task statt Message-Store, lokale SQLite-Persistenz, ein Mutator, explizite Scope-Aktivierung, keine automatischen Worktrees, Factory-Schreibwurzel in der Instanz-`.factory/`. Runtime- und Secret-Spezifika bleiben Plugins.

## Als A0 bereits benannte Obligationen

C03, C05, C07, C11, C15, C17, C23, C29. Diese stehen insbesondere in ADR 0002/0003/0007, der in ADR 0014 benannten read-only Doctor-Ausnahme und der kooperativen Trust-Grenze. `benannt` ist ausdrücklich nicht `implementiert`.

| Zusage | Im Arbeitsstand tatsächlich auffindbar | Grenze |
|---|---|---|
| Versuch vor Writer | `factory-task/src/deliver.rs` | Manuelle Übergabe und mutable Outcomezeile, noch keine komplette Saga/API |
| Exklusivität | Transaktionen und Lease-/Assignment-Prädikate | Recovery kann bei fehlender Evidenz Lease lösen |
| Sichere Korrelation | Confidence und optionale Harness-ID | Missing-ID-Fallback liefert `true` |
| Replay | ADR 0003 | `schema.rs` benennt ausdrücklich noch keinen Event Store |
| Plugin-Supervision | ADR 0007 | `HerdrCli::run` verwendet noch `Command::output()` ohne eigene Deadline |
| Aktuelle Policy | Registry-/Delegationsbibliotheken | Historische Intention und konkrete Freigabeversion brauchen eigenen Vertrag |

## Zusätzliche Inkonsistenz aus Quellenabgleich

ADR 0002 nennt für den Socket einen Pfad unter `~/Library/Application Support/...`; die stärkere aktuelle Projektregel erlaubt Factory-Dateien ausschließlich unter der Instanz-`.factory/`. A3 muss Runtime-Dateien ausdrücklich ebenfalls dort verorten und eine spätere ADR-Änderung benennen. Dies ist kein Anlass, die aktuelle Konfiguration umzuschreiben.

## Erste Matrixsicht: S001–S160

Die 160 Karten umfassen 16 Perspektiven und 338 direkte Inzidenzen. A0 benennt alle zugeordneten Obligationen für 9 Karten, einige für 70 und keine für 81. Das ist ein konservativer **Spezifikationslückenindikator**, keine Ausfallquote. Neue Anforderungen wurden absichtlich über den bestehenden Vertragsumfang hinaus formuliert.

Wichtigste Gegenbeispiele: S031–S034 (falsche Gewissheit bei Runtime-Ausfall), S074/S083 (Retry versus Geschäftsfreigabe), S041–S047 (Prozessgrenze versus Ressourcenbegrenzung), S061/S111 (Intention versus mutable Gegenwart), S153/S158 (zwei Betriebswelten).

**Auswahl für A1:** zuerst die lokal entscheidbaren Sicherheitsübergänge und Überlastgrenzen explizit machen. Nicht zuerst weitere Runtimes oder verteilte HA bauen.
