# Revidierter Architekturentwurf A3

**Historischer Entwurf:** Aktuell ist die [modulare Zielarchitektur A7](a7/zielarchitektur.md) mit vollständiger zustandsbasierter Zuordnung der 350 Stressoren. A3 bleibt als damaliger Ableitungsschritt erhalten; seine Residue-Verdichtung ist keine abschließende Gesamtmenge.

**Status:** Ergebnis des beauftragten Residuality-Verfahrens; zur Ratifikation und Umsetzung, nicht Behauptung vorhandener Implementation. Ersetzt für dieses Analysepaket A0–A2; ändert keine akzeptierten ADRs automatisch. Ableitung: [Iterationen](README.md), [Inzidenzmatrix](generated/incidence-matrix.csv), [Traceability](generated/traceability.csv).

## 1. Architekturthese

Factory ist eine **lokale, evidenzgebundene Koordinationsinstanz**. Sie verwaltet Arbeit und beschränkt die Befugnis zur nächsten Wirkung anhand dessen, was sie aktuell belegen kann. Automation ist ein erlaubter Betriebsmodus, kein Selbstzweck. Auch sicherer Halt, lesbarer Export und bewusstes Nicht-Unterstützen sind gültige Restbetriebe.

Drei fundamentale Trennungen:

1. **Historische Wahrheit ≠ aktuelle Außenwelt.** Replay rekonstruiert Fakten, bestätigt keine laufenden Prozesse und legitimiert keine neuen Effekte.
2. **Ausführungsintention ≠ gültige Berechtigung.** Gepinnte Inputs und alte Freigaben erklären Vergangenheit, überstimmen aber keine aktuelle Policy.
3. **Abschluss ≠ Qualität ≠ externe Freigabe.** Diese Fakten haben verschiedene Produzenten, Evidenz und Folgen.

## 2. Struktur — keine 16 neuen Services

```text
Menschen / Agents / CLI / optionale Clients
          │
   API-Transport-Plugins (Actorbindung)
          │
┌─────────▼──────────────── EIN DAEMON ───────────────────┐
│ Minimal-Bootstrap + Instanzinkarnation + Single Writer  │
│                                                         │
│ Command-Gate:                                           │
│   Auth → aktuelle Policy → Admission → Precondition     │
│                                                         │
│ Kernel-Domäne:                                          │
│   Scope / Agent / Session / Task / Context              │
│   bestehende Run-/Schedule-/Approval-/Saga-Verträge     │
│   Evidence Gate / Workspace-Leases                      │
│                                                         │
│ Transaktion: Events + Idempotenz + Projektion + Intent  │
│                                                         │
│ Kontrollsicht: Unsicherheit / Budgets / Entscheidungen  │
│ Dispatch-Gate: aktuelle Version + Freigabe + Inkarnation│
└──────────┬──────────────────────────────────────────────┘
           │ überwachte begrenzte Pluginaktivierungen
           ├── Runtime/Harness → Session / Prozess
           ├── Effektparticipant → externe Businessoperation
           ├── Content/Vault → sichere Referenzen
           └── Recovery/Export → genehmigte Recovery-Menge

Replayer ── nur gespeicherte Fakten/Inhalte → Projektionen
            keine Plugins, Timer, Workers oder Effekte

Bundled lokaler Kontrolltransport bleibt ohne optionale Plugins.
Offline-Doctor: ausdrücklich read-only, kein zweiter Mutator.
```

R01–R16 sind Restfähigkeiten über diese bestehenden Komponenten hinweg. Kein neuer Message-Store, kein allgemeiner Workflowengine, keine Abhängigkeit von Herdr im Kerndomänenmodell. Runtime-spezifische IDs bleiben opake Adapterreferenzen an der Integrationsgrenze; der Kern braucht einen generischen Korrelations-/Evidenzvertrag statt Herdr-Feldsemantik.

## 3. Zustands- und Vertragsänderungen

Die folgenden Feldnamen sind logische Skizzen, kein vorweggenommenes Migrationsschema.

### 3.1 Evidenz für Sessionübergänge — R01/R03

Eine Beobachtung trägt mindestens: logische Sessionbindung, Runtime-/Harness-Generation oder gleichwertigen Identitätsbeleg, Herkunft/Autorität, beobachtete Lebendigkeit und Aktualität. Ob eine Timestamp sinnvoll vergleichbar ist, gehört zum Adaptervertrag.

```text
may_promote = same_identity AND authoritative AND alive AND fresh
may_release = positive_end_evidence OR explicit_human_resolution
```

`explicit_human_resolution` dokumentiert die konkrete Ende-/Risikoentscheidung und ihren Actor; es ist kein Beweis, dass der Prozess tatsächlich tot ist. Wenn Sicherheit nicht belegbar ist, ist ein anderer **bereits existierender und zulässiger** Workspace die mögliche menschliche Alternative. Factory erstellt, säubert oder verschmilzt keine Worktrees.

- Fehlende ID passt nicht automatisch. Ein Adapter kann einen anderen verifizierbaren Identitätsbeleg anbieten.
- `Unavailable`, `Degraded`, widersprüchliche oder veraltete Beobachtung hält `disconnected` und Lease.
- Ein abgelaufener Timer oder verschwundener Parentprozess allein gibt keine Schreibfreigabe für möglicherweise lebende Nachkommen.
- Ein neuer autoritativer Hook kann über die falsche Session berichten; Autorität ersetzt Identität nicht.
- Readiness, Belegung und Taskqualität bleiben verschiedene Prädikate.

### 3.2 Arbeitspaket und historische Grenze — R04/R05/R14

Ein Run hält: Zielscope/-agent, Task-/Templateversion, sichere Inputreferenzen mit Integritätsmetadaten, tatsächliche Contextrevision/-hash, Executor-/Harness-/Instrumentversion soweit bekannt, Akzeptanzkriterien und Ergebnisevidenz. Unbekannt bleibt unbekannt und wird nicht mit heutigen Werten rückgefüllt.

Konfiguration und menschliche Quellen bleiben menschlich kanonisch. Die für einen historischen Übergang benötigten **aufgelösten Werte** werden als Fakten oder sichere immutable Referenzen eingefroren; Replay läuft nicht gegen die heutige Registry oder heutiges `AGENTS.md`. Kann Inhalt nicht zulässig erhalten werden, wird der Run nicht als vollständig reproduzierbar bezeichnet.

Modell-/Empfängerwechsel erzeugt eine explizite neue Ausführungsentscheidung bzw. einen Ersatzrun mit Herkunftslink. Kein stiller Retry mit anderer Semantik. Native Harnesskonversation ist Komfort, nicht Voraussetzung für Taskverständnis. R14 erhält Workerabgabe und unabhängiges Verifikationsurteil getrennt; ADR 0020s v1-Grenze zur Bench bleibt bestehen.

### 3.3 Effekt- und Freigabevertrag — R02/R09

Task-ID, Run-ID, Attempt-ID, Businessoperation-ID und Teilnehmer-ID sind getrennte Identitäten. Ein fachlich gleicher Effekt kann aus mehreren Tasks kommen; diese Gleichheit braucht einen Domänenvertrag, keine Textähnlichkeitsheuristik.

Vor Dispatch werden durable Intent und nötige Evidenz atomar committet. Freigabe benennt Actor, Payload-/Contentrevision, Teilnehmer/Empfänger, Gültigkeit, zulässige Wirkung und relevante externe Versionpreconditions. Die erneute Prüfung liegt möglichst nahe an der externen Annahme; wo möglich prüft der Teilnehmer dieselben Preconditions atomar.

- Resume erlaubt genau den dokumentierten weiteren Promptversuch, nicht pauschal alle Geschäftsaktionen erneut.
- Verlorene Antwort ergibt `unknown`, nicht bewiesenes `failed`.
- Teilnehmer ohne Idempotenz oder Rekonziliation erhalten keinen Exactly-once-Anstrich.
- Widerruf stoppt noch nicht angenommene neue Schritte. Bereits angenommene Wirkung bleibt Tatsache; Kompensation ist ein neuer spezifischer Vorgang.
- Kompensation braucht aktuelle Versionsprüfung und eigene Wirkungsevidenz; sie überschreibt nicht fremde Zwischenänderungen.
- Ist Persistenz nicht möglich, wird keine neue Wirkung legitimiert und kein dauerhafter Stopp fälschlich bestätigt.

### 3.4 Recovery-Inkarnation — R01/R03/R04/R11

Explizites Restore erzeugt einen Recoverybericht und eine neue Inkarnation; Replay selbst erzeugt keine Runtime. Ein alter Snapshot kann spätere Tasks, Versuche und Widerrufe nicht kennen. Deshalb startet Restore im Rekonziliationsmodus, einschließlich historisch `queued` erscheinender Arbeit.

**Externe Generation nicht blind lokal erfinden:** Ein neuer UUID-Wert ist nützlich zur Herkunft, aber kein Fencing. Automatischer Wechsel zwischen Klonen verlangt einen Teilnehmer oder eine gemeinsam vertrauenswürdige Autorität, die alte Generationen wirklich verweigert. Ohne dies: Single-active-Betrieb mit explizitem menschlichem Ausschluss konkurrierender Writer. Ein zweiter lokaler Daemon scheitert weiterhin am Instanz-Lock.

Ein per Hand kopierter Snapshot ist nicht sicher aus seinem Inhalt als solcher erkennbar. Ein unabhängiger laufender Teilnehmer/Marker kann Verdacht liefern; ohne unabhängige Evidenz verspricht A3 keine automatische Erkennung. Der vorgeschriebene Restore-Prozess und manuell auslösbare Rekonziliation bleiben notwendig.

### 3.5 Betriebsmodi — R07/R08/R10

Dies sind orthogonale Betriebs-/Dispatch-Einschränkungen, **keine neue Task-State-Machine**.

| Modus | Erlaubt | Nicht erlaubt | Austrittsbedingung |
|---|---|---|---|
| Normal | Zugelassene Arbeit innerhalb Budgets | Wirkung ohne aktuelle Freigabe | Störung wechselt in engeren Modus |
| Eingeschränkt | Nicht betroffene Scopes und kontrollierte Teilkapazität | Neue Arbeit über Limits / in quarantänisierte Integration | Last/Evidenz innerhalb festgelegter Grenzen |
| Rekonziliation | Lesen, Evidenz erfassen, explizite menschliche Entscheidungen durch Daemon | Automatischer Dispatch unbekannter historischer Arbeit | Identität, Befugnis, Operation und Ressourcen ausreichend geklärt |
| Inspektion | Read-only lokale Diagnose und zulässiger Evidenzexport | Core-Mutationen bei fehlendem Writer/inkompatiblem Store | Expliziter valider Start/Recovery; kein Reparaturautomatismus |

Sperren können enger auf Scope/Aktivierung begrenzt werden, wenn die gemeinsame Autorität und Persistenz gesund sind. Bei beschädigtem Store ist ein „nur dieser Scope“-Versprechen nicht begründbar. Die Hierarchie der Modi ist keine Garantie, bei physischem Ausfall überhaupt Code ausführen zu können.

### 3.6 Ressourcen und Zeit — R06/R07/R13

Budgetkategorien: zugelassene Tasks/Bytes, aktive Pluginprozesse und Nachkommen, Frames/Output, Subscriberqueues, parallele Sessions, Kosten, disk usage und Restartversuche. Zusätzlich `max_sessions` pro Agent beibehalten. Fairness und globale Caps sind getrennt von lokaler Parallelität.

Keine willkürlichen Produktionszahlen erfinden: vor Rollout legt der Owner/Operator Grenzen fest und T07/T08/T14 prüfen sie bei übersteigender Last. Ein Teil CPU/FD/Queue-/Speicherbudget bleibt für Kontrolle reserviert. Vor Erschöpfung neue Aufnahme/Dispatch stoppen, disposable Beobachtungen begrenzen; kanonische Historie oder einziges Backup nicht automatisch löschen.

Schedule-Occurrence benennt Scheduleversion, fachlichen Termin und explizite DST-/Catch-up-Regel. UTC kann eine konkrete Occurrence identifizieren, entscheidet aber nicht, ob zwei Wiederholungen derselben lokalen Stunde fachlich beide gewünscht sind. Monotone Timeouts, Geschäftszeit, Inputverfall und Freigabegültigkeit sind getrennte Verträge. Eine Minute Dispatcher-Takt ist keine Geschäftsdeadline.

### 3.7 Menschliche Steuerung, Datenschutz und Ausstieg — R10/R11/R12/R15/R16

- Entscheidungen zeigen Scope/Instanz, betroffene Operation, Evidenzalter, Unsicherheit, reversible Optionen und Folgen des Nichtstuns. Maximal verständliche Bündelung, aber keine Sammelfreigabe mit versteckten individuellen Effekten.
- Vertretung, Befangenheit und Nachfolge werden ausdrücklich legitimiert. Ohne legitime Autorität sicher halten. Selbst ein informierter Mensch ist nicht unfehlbar.
- Events tragen minimale sichere Metadaten. Sensibler Inhalt gehört unter kontrollierte Referenzen, Secrets ausschließlich hinter den Vault-Vertrag. Rohtranskripte in Produktion werden nicht durch diese Analyse autorisiert.
- Retentionklassifikation trennt kanonische Fakten, benötigte Inhalte, Trace-Artefakte und disposable Samples. Löschung/Legal Hold/Replaykonflikt erhält einen benannten Entscheider und dokumentierte Evidenzgrenze. Kein stilles Umschreiben alter Events.
- Recoveryinventar umfasst DB/Events, Idempotenz-/Operationfakten, benötigte Inhalte, Konfigurations-/Formatversionen, Wiederaufbauanleitung und getrennten Schlüssel-Recovery-Prozess. Backups in unabhängiger Ausfalldomäne werden nur nach Freigabe eingerichtet. Zunächst kann das ein Operatorprozess sein, kein neuer Factory-Cloud-Dienst.
- Factory-eigene Dateien einschließlich Socket, Logs, Runtimeverzeichnis und erzeugtem Export liegen in der **Instanz-`.factory/`**. Kein projektspezifisches `.factory/`, kein bloßes Match auf irgendeinen Pfadbestandteil namens `.factory`. OS-/Vault-Integration bleibt hinter ihrem Vertrag. Unabhängiges Backup darf kein verdeckter Factory-Dateischreibpfad außerhalb dieser Wurzel werden.
- Abhängigkeiten und Formate haben Versions-/Herkunftsevidenz und kompatible Fixtures. Stilllegung liefert zulässigen lesbaren Export und offene Effekte/Retentionpflichten; sie löscht nicht automatisch alles.
- v1 bleibt Trusted Single User. Feindliche Mandanten, Safety-Echtzeit und Active-active sind neue Mandate und werden nicht aus dem Pluginmodell abgeleitet.

## 4. Warum diese Grenzen zusammengehören

Die Inzidenzmatrix zeigt R02/R09 auf 16 gemeinsamen Stressoren: Effektabsicht ohne aktuelle Freigabe ist gefährlich, Freigabe ohne dauerhafte Effektidentität nicht auditierbar. R06/R07 teilen acht Karten: Prozess-Supervision ohne Gesamtbudget kann das gemeinsame Betriebssystem erschöpfen. R04/R11/R16 verbinden Historie, Recovery und Formatfortbestand. Darum sind getrennte Komponenten **nicht** mit unabhängigen Ausfalldomänen zu verwechseln.

Die expliziten Residue-Voraussetzungen stehen in [residue-dependencies.csv](residue-dependencies.csv). R08 und R03 haben jeweils nur zehn direkte Inzidenzen, aber eine große indirekte Bedeutung. Rein nach Spaltensummen zu priorisieren wäre falsch.

## 5. Änderungen an bestehenden Verträgen zur Ratifikation

| Bestehende Quelle | A3-Änderung/Präzisierung | Warum nicht still übernehmen? |
|---|---|---|
| ADR 0002 | Runtime-/Socketpfade unter Instanz-`.factory/`; lokaler Kontrollpfad vor optionaler Konfigvalidierung | Alter Socketpfad widerspricht stärkerer aktueller Schreibregel; Bootreihenfolge ist sicherheitsrelevant. |
| ADR 0003 | Freigabe-/Inkarnationsbindung, historische Referenzgrenze, expliziter Cutover zum kanonischen Eventmodell | Mutable aktuelle Tabellen können nicht einfach „Projektionen“ umbenannt werden. |
| ADR 0007 | End-to-End-Ressourcengrenzen, gemeinsame Budgets, tatsächliche Fencing-/Reconciliation-Fähigkeit | Manifestbehauptung ist kein Teilnehmernachweis. |
| ADR 0012/0017 | Positiver Identitäts-/Endebeleg und Aktualität für Promotion/Release | Missing-ID-Match und Leasefreigabe bei Unavailable im Arbeitsstand widersprechen dem vorgeschlagenen Vertrag. |
| ADR 0019 | Restore-Epoche mit Dispatch-Sperre auch für historisch queued/no-attempt; Recovery-Set statt DB-Snapshot als Vollständigkeitsbegriff | Stärkere Semantik als „queued ohne Versuch bleibt queued“; Taskstatus darf bleiben, Dispatchbefugnis nicht. |
| ADR 0013/0020 | Historische sichere Kontext-/Inputbindung und tatsächliche Instrumentrevisionen, heutige Policy bleibt maßgeblich | Keine neue automatische Retrieval-/Bench-Plattform in v1. |
| Backlog Slices 9–13 / Task-Migrationsplan | Gates für Recovery, Cutover, Budgets und Referenzretention explizit einordnen | Kein paralleler dauerhafter Implementierungsbacklog in diesem Ordner. |

## 6. Akzeptanz- und Migrationsgrenze

Vor automatischer Runtime-Recovery: T01–T04 mit Gegenbeispielen gegen Missing-ID, Unavailable, Crashfenster, Snapshotverlust und Klone. Vor unbeaufsichtigtem Betrieb: T05–T14 und T17 für kanonische Persistenz, Kontrollpfad, Last, aktuelle Freigaben, Recovery und Cutover. Fachliche externe Nutzung braucht passende T15-Gates. Eine Erweiterung des Trustmodells braucht T16 plus eigenes Isolationsmandat.

Der heutige Rust-Store hat noch keinen Event Store. Migration muss aktuelle Fakten als explizite Importbaseline erhalten, nicht verlorene Geschichte erfinden; Prototyp und Rust niemals unkontrolliert parallel mutieren lassen. Reale Umstellung benötigt eigene freigegebene Aufgabe mit Backup, Pause, Prüfung und Rollback. Dieses Dokument startet sie nicht.

**Nicht versprochen:** lückenlose Historie nach Daten-/Schlüsselverlust, vollständige Erkennung manueller Snapshotkopien, globale Exactly-once-Wirkung, universelle Ergebniswahrheit, harte Isolation im gleichen Useraccount, juristische Widerspruchsfreiheit oder unbegrenzte Verfügbarkeit.
