# P02 — erste ausgeführte Teilprüfung

Stand: 2026-09-09. Run: `1667b729-ed01-41b0-b042-f5b07da02fa6`. **Vier lokale Rust-Integrationstests bestanden. P02 als Ganzes bleibt offen.**

## Was wirklich ausgeführt wurde

[Neue Testdatei](../../../../../crates/factory-recovery/tests/residuality_p02.rs) · [Testausgabe](test-output.txt) · [Quellinventar vor der Ausführung](source-hashes.json)

Getestet wurden echte `Store::backup_to`-Sicherungen mittels SQLite `VACUUM INTO`, das Öffnen isolierter Kopien, `deliver`, `restore::reconcile` und `authorise_resume`. Nur der Empfänger ist ein lokales Testdouble: Er behält den synthetischen Prompt im Speicher und meldet danach einen Fehler — oder nimmt nichts an und meldet denselben Fehler. Keine Geschäftsaktion, kein echter Teilnehmer, kein Herdr und kein Daemon wurden gestartet.

Die Kopien stammen aus abgeschlossenen Snapshotdateien, **nicht** aus einer laufenden WAL-Datenbank. Alle Teststores und Restoreberichte liegen unter temporären `.factory/`-Verzeichnissen. SQL dient nur zum Aufbau und Lesen isolierter Fixtures, nicht zum Zugriff auf den operativen Factory-Taskstore.

| Test | Beobachtung | Eng begrenzte Aussage |
|---|---|---|
| Versuch erhalten, Prompt im Testdouble angekommen, Antwort verloren | Das Journal enthält `failed`; zweiter direkter Versuch wird verweigert. Wiederherstellung hält den Task, trennt die Session und behält deren Lease. | Ein erhaltener Versuch schützt auf diesem Pfad vor blindem erneuten Writer-Aufruf. `failed` beweist keine Nichtannahme. |
| Versuch erhalten, Prompt nicht angekommen | Derselbe Halt trotz null empfangener Prompts. | Die Schutzreaktion unterscheidet den unbekannten Empfang nicht durch Raten. Sie kann absichtlich auch noch nicht ausgeführte Arbeit halten. |
| Sicherung vor dem Versuch, Wiederherstellung vor beziehungsweise nach späterem Empfang | Beide Kopien liefern denselben Taskzustand `queued`, kein Journal und Budget 1. In einer Welt gab es noch keinen Empfang, in der anderen einen. | Fehlender Versuch im alten Snapshot ist kein Beweis, dass danach nichts gesendet wurde. |
| Fortsetzung eines gehaltenen Tasks | Budget steigt von 1 auf 2; Journal bleibt unverändert, Zuweisung wird gelöscht, Writer wird nicht aufgerufen und Lease bleibt gehalten. | Neue Zustellmöglichkeit, Wissen über vorherigen Empfang und Workspacebesitz sind verschiedene Gegenstände. |

**Wichtige Gegenbegrenzung:** Im alten Snapshot wird die Zuweisung gelöscht. Ein direkt nachfolgender `deliver`-Aufruf endet mit `NotAssigned`; im Test gibt es keinen zweiten Empfang. Das Ergebnis ist daher **kein nachgewiesener automatischer Doppelversand**, sondern ein konkret ausgeführter Gegenbeleg zur Begründung „Nothing was ever sent“ in ADR 0019. Eine spätere Neuzuweisung, Laufzeitprüfung und echte Teilnehmerannahme wurden nicht durchgespielt.

Der Test zum alten Snapshot charakterisiert die heutige Grenze. Er verlangt nicht, diese Grenze dauerhaft zu erhalten: Ändert eine ratifizierte Restore-Lösung den Vertrag, muss seine Erwartung ausdrücklich mitgeändert werden.

## Was als Residue tatsächlich getragen wird

- **M01/M06:** Der erhaltene Versuch ist lesbar und verhindert im getesteten Pfad einen weiteren Aufruf ohne neue Zustellmöglichkeit.
- **M05/M09:** Die getrennte Session behält die lokale Workspace-Lease. Das ist weder Nachweis ihres Todes noch Ausschluss eines Schreibers auf einem anderen Rechner.
- **M06:** Fortsetzen verändert den Zustellzähler atomar durch die vorhandene Funktion, aber liefert keine neue Erkenntnis über den vorigen Empfang. Der neue Test ist keine parallele Belastungsprüfung dieser Atomizität.
- **Nicht erhalten im alten Snapshot:** die darin noch nicht enthaltene spätere Versuchsevidenz. Ihre Existenz darf nicht aus der alten Datei rekonstruiert werden.

Das grenzt insbesondere die A7-Verträge um OPR002, OPR003 und KOR001 ein. Der heutige Zähler ist **nicht** bereits KOR001s vollständige revidierte, aktuell gültige Autorisierungsstruktur.

## Der Widerrufsteil ist noch kein ausgeführter Vertrag

`authorise_resume(store, task_id)` erhält keine aktuelle Principal-/Grant-/Widerrufsevidenz. Auch der inspizierte Daemonpfad `ops/task.rs::resume` übergibt nur Store und Task-ID. Daraus folgt eine Grenze dieser untersuchten Schnittstellen, **keine vollständige Aussage über sämtliche Zugangsprüfungen oder die Sicherheit des lokalen Transportes**.

Ein erfundener boolescher Wert `revoked = true` im Test wäre keine Prüfung einer implementierten AuthoritySource. Deshalb wird kein solcher Scheintest als Erfolg gezählt. GVR011s frische unabhängige Autoritätsquelle, eine tatsächlich zurückgewiesene widerrufene Aktion und die Trennung von Wiederaufnahme- und Geschäftsfreigabe bleiben gesondert nachzuweisen. P02s vollständige Kombination ist nicht bestanden.

## Bezug zur Architekturentscheidung

Der aktuelle Code folgt in `restore.rs` der akzeptierten [ADR 0019](../../../../adr/0019-restore-and-what-a-recovered-database-may-claim.md). Deren unbedingte Begründung für einen alten Snapshot ohne Versuch wird durch den lokalen Gegenfall nicht getragen. Die ADR wurde hier **nicht geändert**.

Ein nächster begrenzter Entscheid muss klären:

1. Wie werden normale Neustarts und ausdrücklich erklärte beziehungsweise anderweitig belegte Rücksetzungen unterschieden? Der inspizierte Daemon-Start ruft ebenfalls `reconcile` auf; ein pauschaler Halt jeder frischen Queue hätte zusätzliche Folgen.
2. Welche unbekannte Nachgeschichte muss eine Rücksetzung sichtbar erhalten, statt negative Fakten zu erfinden?
3. Woher kommt die aktuelle Erlaubnis? Fehlende frische Quelle bedeutet keine neue positive Freigabe.
4. Welcher reale Teilnehmer kann einen früheren Effekt belegen oder einen alten Auftrag tatsächlich abweisen?

Erst nach dieser Entscheidung wäre eine vertikale Implementierung sinnvoll. Ein neuer Enumwert allein ersetzt diese Verträge nicht.

## Quellenstand und Reproduktion

Ausgangspunkt: HEAD `e93597f4145306a51c1867f1dfb9a2a505df7383` **plus bestehende Änderungen**, nicht HEAD allein. Anders als in früheren Analysestufen existiert inzwischen zusätzlicher Daemon-/CLI-Code. Diese vier Tests führen ihn nicht aus.

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo test --offline --locked \
  -p factory-recovery --test residuality_p02
```

[Erfasste Umgebung](environment.txt): macOS 26.6.1, ARM64, Rust/Cargo 1.98.1. Dateisystem-Dauerhaftigkeit und Stromausfall wurden nicht geprüft. Rust war vorhanden, aber zunächst nicht im Shell-PATH. Kein Toolchain- oder Dependency-Download wurde vorgenommen. Vorhandene Produktdateien, Manifeste und Fixtures wurden nicht bearbeitet.

Zusätzlich vorbereitet: [Mutationsgegenprobe](mutation_probe.py). Sie soll ausschließlich in einer gewöhnlichen temporären Quellkopie den Journalguard abschalten und erwartete Testfehler prüfen. **Noch nicht ausgeführt:** Die Quellenprüfung brach vor Kompilierung des Mutanten ab, weil andere Arbeit inzwischen Quellen verändert hatte ([erster Abbruch](mutation-preflight.txt), [zweiter Abbruch](mutation-output.txt)). Zunächst betraf das unbeteiligte Testtargets; nach deren begründeter Ausklammerung auch `factory-adapter/src/claude.rs`. Die Änderungen wurden nicht zurückgesetzt und die Prüfbasis nicht still neu festgelegt.

Das Quellinventar ist eine zeitgebundene Aufnahme. Es ist kein Versprechen, dass die gegenwärtig weiterentwickelten Dateien noch identisch sind. Die Mutationsgegenprobe benötigt einen erneut geprüften ruhigen Stand. Es gibt hier **vier positive Testausgänge, keinen bestandenen Mutationstest, keinen Windows-/Linux-Lauf und keine Betriebsfreigabe**.
