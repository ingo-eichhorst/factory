# Factory: Stressor-Analyse in Anlehnung an Residuality Theory

- Stand: 2026-09-08; erste explorative Iteration, zur Diskussion.
- Bezug: Barry O’Reillys Residuality Theory; eigenständige Anwendung, keine wortgetreue Wiedergabe seiner Methode.
- Grundlage: Repository bei `e2b5d25` **einschließlich vorhandener, uncommitteter Änderungen**. Insbesondere `factory-recovery` ist noch untracked/in Arbeit.
- Task-Run: `458e06c1-c6bb-4df9-8e80-0e1d96242dd1`.
- Evidenz: Dokumenten- und Codelektüre. Keine Tests ausgeführt, keine Ausfälle injiziert, keine Live-Systeme verändert.
- Status: Analyse und Vorschläge, **kein ADR, keine genehmigte Änderung des Lieferplans**.

## 1. Leitfrage und Methode

Nicht: „Welche vorhersehbaren Fehler fangen wir jeweils ab?“

Sondern: **„Welche Architektur bleibt unter sehr unterschiedlichen Veränderungen nützlich, und welche Reststrukturen können wir zu einer tragfähigeren Architektur zusammensetzen?“**

Mit *Residual* meinen wir hier die nach einem Stressor noch nutzbare Reststruktur einschließlich ihrer verbleibenden Fähigkeiten und Grenzen. Ein Retry, ein Backup oder ein Circuit Breaker allein ist noch kein Residual. Entscheidend ist, welches System damit weiter existieren kann und was es noch ehrlich verspricht.

Vorgehen dieser ersten Iteration:

1. Den heutigen Ausgangszustand und seine Annahmen festhalten.
2. Technische, organisatorische, wirtschaftliche und sicherheitsbezogene Stressoren anwenden — auch solche außerhalb des geplanten v1-Einsatzes.
3. Pro Stressor gebrochene Annahme, verbleibende Fähigkeit und mögliche strukturelle Anpassung bestimmen.
4. Wiederkehrende Anpassungen bündeln und auf Stressor-Kombinationen anwenden.
5. Kandidaten mit weiteren Gegenbeispielen angreifen; anschließend messbare Experimente formulieren.

**Keine Wahrscheinlichkeits-/Schadensmatrix:** Die Auswahl soll versteckte Kopplungen sichtbar machen, nicht eine vermeintlich vollständige Liste wahrscheinlicher Vorfälle liefern. Die Stressoren wurden bewusst gewählt, nicht zufällig gezogen. Die Gegenproben sind Gedankenexperimente desselben Autors, kein unabhängiger Holdout und kein Nachweis von Sättigung oder allgemeiner Resilienz. Eine nächste Runde sollte zusätzliche Stressoren von Menschen einbringen, die die Architektur nicht entworfen haben.

## 2. Welches System setzen wir unter Stress?

### Drei Zustände, nicht eine fertige Factory

| Ebene | Tatsächlicher Bezug | Was daraus nicht folgt |
|---|---|---|
| Operativer Prototyp | Company-root Python-Task-System, Agent-Tool und Dispatcher laut Migrationsplan; dieses Task-Tool begleitet auch die Analyse. | Dass der Rust-Daemon bereits diese Arbeit übernimmt. |
| Rust-Arbeitsstand | Crates für Konfiguration, Registry, Context, Adapter, Sessions, Tasks, Delegation, Store und Recovery. Manuelle Prompt-Übergabe; Pi-Beobachtung über Herdr-CLI. | Dass ein vollständiger Daemon, Plugin-Host, CLI oder automatischer End-to-End-Lifecycle geliefert ist. |
| Architekturvertrag | Daemon als einziger Mutator; Event Store und CQRS; isolierte Plugin-Prozesse; lokale Recovery; Sagas und Freigaben. | Dass diese Schutzwirkung bereits implementiert oder experimentell bestätigt ist. |

Das README behauptet noch „Slices 1, 2, 4 implementiert“, obwohl weitere Crates vorliegen. Der Backlog nennt Resume noch ungelöst, während der Arbeitsstand `authorise_resume` enthält. Umgekehrt macht ein Crate-Eintrag noch keinen fertigen Slice. Deshalb gelten konkrete Fundstellen mehr als pauschale Fortschrittsangaben.

### Vorläufiges Überlebenskriterium

Factory bleibt nützlich, wenn es auch im degradierten Betrieb:

- bereits angenommene Arbeit und deren Evidenz auffindbar hält;
- keine möglicherweise ausgeführte Aktion blind wiederholt;
- keinen möglicherweise belegten Workspace neu vergibt;
- Unsicherheit sichtbar macht statt Erfolg, Tod oder Identität zu erfinden;
- externe Aktionen nicht aus einem Schedule, Replay oder Resume heraus neu legitimiert;
- einem Menschen einen verständlichen Weg zu Diagnose und expliziter Fortsetzung lässt.

**Verfügbarkeit der Automatisierung darf sinken; Sicherheit und Wahrhaftigkeit der Aussagen nicht.** Wie lange Stillstand wirtschaftlich tragbar ist, bleibt eine Nutzerentscheidung. Auch das Halten aller Leases ist kein dauerhaft tragfähiger Geschäftsbetrieb.

## 3. Stressor-Karten

Die Restfähigkeiten sind statische Ableitungen, keine bestandenen Tests. Hinweise auf den Entwurf sind entsprechend markiert.

| ID | Stressor und gebrochene Annahme | Was bleibt / was verloren geht | Kandidat für die nächste Architektur |
|---|---|---|---|
| S01 | Der Daemon stirbt direkt nach Prompt-Übergabe, vor deren Bestätigung. „Keine Antwort heißt nicht ausgeführt“ ist falsch. | Der vorab persistierte Zustellversuch bleibt, sofern sein Commit erhalten ist. Der tatsächliche Empfang bleibt unbekannt. | Zustellabsicht und Effektidentität dauerhaft halten; unbekannte Ergebnisse rekonzilieren, nicht automatisch erneut senden. |
| S02 | Herdr antwortet nicht, aber der Harness schreibt weiter. Beobachtbarkeit ist nicht Prozesslebendigkeit. | Task-Journal und letzte Zuordnung bleiben. Ein sicherer Beleg für das Ende fehlt. Der aktuelle Recovery-Pfad kann dennoch die Lease freigeben, siehe F01. | Restbetrieb mit gesperrtem Workspace und offener Evidenzfrage; Freigabe nur nach positivem Beleg oder expliziter menschlicher Entscheidung. |
| S03 | Pane-ID wird wiederverwendet, Transcript-Dateiname ändert sich, Session-ID fehlt. „Dieselbe Adresse heißt dieselbe Identität“ bricht. | Registry und Task bleiben; Pane-Korrelation allein trägt die Wiederaufnahme nicht. | Identität, Beobachtungsautorität und Aktualität getrennt nachweisen. Unvollständige Identität darf nicht automatisch passen. |
| S04 | Plugin hängt, schreibt endlos Logs oder liefert einen riesigen Frame. Prozessgrenze ist keine Ressourcenisolation. | Mit dem geplanten Host können andere Aktivierungen weiterlaufen — nur bei wirksamen Limits. Der jetzige CLI-Aufruf hat selbst keine Deadline. | Begrenzte Laufzeit, Bytes, Warteschlangen und Restart-Budgets; Control-/Diagnose-Kapazität reservieren. |
| S05 | Nach langem Sleep werden viele Cron-Zeitpunkte fällig; Uhr springt zurück oder die lokale Minute wiederholt sich bei DST. | Dauerhafte Tasks können erhalten bleiben, aber Nachholarbeit kann Kapazität und Operator überfordern. Eindeutigkeit eines lokalen Minutenlabels entscheidet noch nicht die gewünschte DST-Semantik. | Explizite Occurrence-Identität und Catch-up-Regel; Aufnahmebegrenzung und faire Verteilung, keine unbegrenzte Nachholwelle. |
| S06 | Alle temporären Worker warten auf Freigaben; der einzige Operator ist drei Tage abwesend. „Manuelle Intervention ist jederzeit verfügbar“ bricht. | Sicherheitsgrenzen bleiben sinnvoll, Arbeit kommt aber nicht voran; blockierte Worker halten Leases und Slots. | Unklarheiten lesbar bündeln, neue Aufnahme bremsen, Diagnosezugang erhalten. Kein automatisches Zustimmen, keine Freigabe einer Lease wegen Zeitablaufs. |
| S07 | Modellanbieter verschwindet oder verteuert sich massiv. Ein vorhandener Harness ist keine dauerhaft bezahlbare Ausführungskapazität. | Aufgaben, Kontextquellen und Ergebnisse können Anbieterwechsel überleben. Native Konversationen und gleiche Ergebnisqualität nicht. | Providerunabhängige Arbeitsbeschreibung und Abnahmekriterien; Modellwechsel als explizit neue Ausführungsentscheidung, nicht als stiller Retry. |
| S08 | Ein Task ist syntaktisch fertig, aber fachlich falsch; der Harness meldet `done`. Lifecycle ist kein Qualitätsbeleg. | Arbeitsergebnis und Worker-Aussage bleiben prüfbar. Automatische Dispatch-Freigabe wäre unbegründet. | Worker-Abschluss, unabhängige Verifikation und externe Freigabe als getrennte Fakten erhalten; keine neue Message-/Thread-Domäne nötig. |
| S09 | Registry oder `AGENTS.md` ändert sich zwischen Queueing, Start und Resume; Zielagent wird umbenannt. Deterministische Funktion heißt nicht historische Reproduzierbarkeit. | Aktuelle Dateien sind lesbar, aber die frühere Ausführungsintention ist daraus nicht rekonstruierbar. `target_agent_name` fehlt im Rust-Task-Schema. | Empfänger und revisionsgebundene Ausführungsintention persistieren. Vor Nutzung alte Evidenz gegen heutige Berechtigung prüfen; veraltete Freigaben nicht durch Pinning konservieren. |
| S10 | Backups und Telemetrie füllen die SSD. „Mehr Audit ist immer sicherer“ bricht. | Bestehende Daten können noch lesbar sein; neue durable Zusagen und Zustellversuche dürfen ohne Commit nicht entstehen. | Speicherbudget und Warnschwellen; vor Erschöpfung Aufnahme stoppen. Nichtkanonische Telemetrie begrenzen; kanonische Historie und einzige Backups nicht still löschen. |
| S11 | Der einzige Mac samt SSD geht verloren. „Local-first“ ist keine getrennte Ausfalldomäne. | Nur außerhalb des Geräts erhaltene Daten überleben. Ein Snapshot auf derselben SSD ist mit verloren. | Definierte Recovery-Menge und genehmigte Kopie in anderer Ausfalldomäne; Schlüssel separat absichern, Wiederherstellung praktisch testen. |
| S12 | Alter DB-Snapshot wird restauriert; seitdem wurde eine externe Aktion ausgeführt oder eine Freigabe widerrufen. Vergangenheit der DB ist nicht Gegenwart der Welt. | Snapshot-Evidenz bleibt, spätere Versuche fehlen vollständig. Auch ein darin `queued` Task ohne Versuch kann in Wirklichkeit schon ausgeführt sein. | Restore als neue Recovery-Epoche; Dispatch zunächst sperren, insbesondere externe Effekte anhand unabhängiger Evidenz rekonzilieren. Snapshot-Leere beweist kein Nichtgeschehen. |
| S13 | Nach Upgrade ist eine Projektion korrupt oder ein altes Event-Payload unbekannt. „Wir können jederzeit replayen“ muss mehr als ein ADR sein. | Beim heutigen Rust-Store sind Task-Zeilen nicht aus Events regenerierbar. Im Zielentwurf bleibt das Log nur dann nutzbar, wenn Reducer und Referenzinhalte verfügbar sind. | Erst einen vollständigen replaybaren Task-Pfad samt Upcast-/Referenzvertrag nachweisen; unbekannte Version stoppt explizit, nie überspringen oder Effekte auslösen. |
| S14 | Python-Prototyp und Rust nehmen gleichzeitig Mutationseigentum an derselben DB an. SQLite-Serialisierung ist kein gemeinsames Domänenmodell. | Der dokumentierte Schema-Konflikt stoppt derzeit die Migration laut Migrationsplan lautstark. Das ist sicherer als stilles Adoptieren. | Ein expliziter Cutover mit einem Writer, nachweisbarer Schema-/Instanzidentität und Rollback; keine improvisierte Dual-Write-Phase. |
| S15 | Ein falsch konfigurierter Scope lädt ein defektes Plugin; ein optionaler HTTP-Transport fällt aus. Optionalität muss auch im Fehlerpfad gelten. | Andere Scopes und lokaler Zugriff sollen laut Entwurf übrig bleiben. Ein gemeinsamer Startpfad könnte trotzdem alles blockieren. | Aktivierungsfehler lokal quarantänisieren; gebündelten lokalen Kontrollpfad unabhängig von optionalen Manifesten/Plugins booten können. |
| S16 | Ein zukünftiger Kunde will untrusted Repositories oder ein Plugin exfiltriert Daten. Kooperative Policy ist keine Security Boundary. | Audit kann einen Vorfall erklären, aber Same-User-Prozesse verhindern weder Datei- noch Secret-Zugriff grundsätzlich. | v1-Einsatzgrenze explizit beibehalten. Vor feindlichen Workloads separate OS-/VM-/Credential-Grenzen entscheiden, nicht nur weitere Prompt-Regeln ergänzen. |
| S17 | Ein Secret gelangt trotz Regeln in Prompt, Rohfehler oder Ergebnis und damit in dauerhafte Historie. Append-only verstetigt den Fehler. | Metadaten und nicht betroffene Evidenz bleiben brauchbar; ein bloßes Korrektur-Event entfernt das Secret nicht aus DB, Kopien oder Backups. | Prävention vor Persistenz, minimaler Payload, sensible Inhalte über kontrollierte Referenzen. Separater Incident-/Retention-Prozess und Rotation mit Freigabe; Append-only nicht heimlich überschreiben. |
| S18 | Der Maintainer fällt aus; neue Agents implementieren widersprüchliche README-, ADR- und Code-Kommentare. Dokumentationsmenge ist nicht Kohärenz. | Quellcode, Tests und Entscheidungen bleiben, aber ihre Geltung ist schwer erkennbar. Übergangsannahmen werden versehentlich Produktgarantien. | Kleine Status-/Vertragsübersicht mit Verantwortlichkeit; sicherheitskritische Behauptungen an ausführbare Tests binden; Produkt- und Übergangscode klar markieren. |

## 4. Konkrete Befunde im Arbeitsstand

Diese Punkte sind **Review-Befunde aus statischer Lektüre**, keine beobachteten Produktionsvorfälle. Die Recovery-Dateien werden parallel entwickelt; vor einem Patch erneut prüfen.

### F01 — Unsicherheit kann eine Lease freigeben

`reconnect_after_supervisor_restart` in [reconnect.rs](../crates/factory-recovery/src/reconnect.rs) führt bei `Ok(observation)`, das die Promotion-Prüfung nicht besteht, `give_up_on_disconnected_session` aus. Das setzt die Session auf `failed` und gibt die Lease frei. Auch `Unavailable` oder `Degraded` kann so enden.

**Stressor S02:** Herdr ist unerreichbar, der Harness lebt weiter. Aus fehlender positiver Bestätigung folgt nicht, dass der Workspace frei ist. Der beabsichtigte Residual „Arbeit blockiert, Workspace geschützt“ ist damit noch nicht durchgehend vorhanden.

Zusätzlich prüfen die Promotion-Zweige `confidence` und `identifies_same_session`, aber nicht explizit `session_alive`. Die öffentliche `Observation`-Struktur erlaubt diese Angaben unabhängig voneinander. Der Consumer sollte seine notwendige Evidenz nicht nur aus der heutigen Producer-Implementierung voraussetzen.

### F02 — Fehlende Identität wird als Match behandelt

`identifies_same_session` in derselben Datei liefert bei zwei vorhandenen IDs deren Vergleich, sonst `true`. Der Kommentar benennt die Lücke ausdrücklich; ein Test erwartet diesen Fallback sogar.

**Stressor S03:** Ein Hook berichtet autoritativ über die **falsche** Session. Autorität beantwortet „Wie kam die Beobachtung zustande?“, nicht „Ist es unsere Session?“. Ein grüner Test kann hier die unsichere Annahme stabilisieren, statt sie zu widerlegen.

### F03 — Replay-Schutz ist noch Architekturvertrag

[schema.rs](../crates/factory-store/src/schema.rs) sagt ausdrücklich, dass noch kein Event Store existiert. Task-/Session-Änderungen schreiben mutable Zeilen; `deliver` aktualisiert das Outcome eines Zustellversuchs. [ADR 0003](adr/0003-logical-api-and-event-sourcing-v1.md) verlangt dagegen kanonische Events und deterministischen Replay.

**Stressor S13:** Task-Zeilen löschen und „aus dem Audit neu bauen“ ist heute kein zulässiger Recovery-Weg. Ein späteres vollständiges Event-Modell kann vor seiner Einführung nicht aufgezeichnete Fakten auch nicht rückwirkend erfinden. Es braucht eine ehrliche Import-/Baseline-Grenze. Die Geltung für v1 ist mit ADR 0010/Backlog explizit abzugleichen, nicht still zu vertagen.

### F04 — Adaptergrenze noch nicht Host-Isolation

`HerdrCli::run` in [factory-adapter](../crates/factory-adapter/src/lib.rs) nutzt `Command::output()` ohne eigene Deadline oder Output-Grenze. [ADR 0007](adr/0007-supervised-plugin-process-protocol-v1.md) verlangt überwachte Plugin-Prozesse, Framing-Limits und Restart-Budgets.

**Stressor S04:** Der jetzige Bibliotheksaufruf allein trägt diese Garantien nicht. Das ist eine Integrationslücke, kein Beweis dafür, dass der geplante Plugin-Host defekt wäre.

### F05 — Zustellbudget ist noch keine vollständige Freigabe-Evidenz

[deliver.rs](../crates/factory-task/src/deliver.rs) persistiert den Versuch vor dem Writer-Aufruf und begrenzt Versuche anhand von `authorised_deliveries`: ein wertvoller vorhandener Schutz. `authorise_resume` erhöht den Zähler atomar, nimmt aber selbst weder authentifizierten Actor noch revisionsgebundene Freigabe-Evidenz entgegen.

**S01 + S12:** Die spätere API muss die menschliche Autorisierung verlässlich binden und protokollieren. Die Erlaubnis, einen Prompt erneut zuzustellen, ist außerdem **keine neue Erlaubnis für dessen externe Geschäftsaktion**. Auch „at-most-once Prompt-Übergabe“ bedeutet nicht „at-most-once externe Wirkung“.

## 5. Wiederkehrende Residuals und strukturelle Kandidaten

Die folgenden Kandidaten entstehen nicht aus einem einzigen Fehlerszenario. Sie sind Kombinationen von verbleibenden Fähigkeiten und gezielter Verstärkung, keine Forderung nach sechs neuen Subsystemen.

| Residual-Kandidat | Restfähigkeit unter Stress | Unterstützte Stressoren | Kleinste strukturelle Verstärkung / Preis |
|---|---|---|---|
| R1: Evidenzgebundene Arbeit | Tasks und Leases bleiben verständlich, obwohl der Runtime-Zustand unbekannt ist. | S01–03, S06, S12 | Promotion nur bei ausreichender Identität, Lebendigkeit und frischer Evidenz; Unbekannt hält die Lease. Preis: mehr sichtbarer Stillstand und notwendige manuelle Auflösung. |
| R2: Minimaler Kontrollbetrieb | Lokal inspizieren und weitere Arbeit sicher anhalten, während optionale Automation fehlt. | S04, S06, S10, S15, S18 | Safe-mode-IPC plus der in ADR 0014 benannten read-only Offline-Diagnose; keine zweite Mutationsinstanz. Explizite Aufnahme-/Dispatch-Sperren benötigen keinen freien Worker. Preis: eigener getesteter Recovery-Bootpfad. |
| R3: Dauerhafte Effektabsicht | Arbeit kann zwischen Prozessen und Anbietern wechseln, ohne ihre mögliche Außenwirkung zu vergessen. | S01, S07, S12–14 | Task, Zustellversuch, externe Operation und Freigabe getrennt korrelieren; persistente Operation-ID und participant-seitige Rekonziliation gemäß ADR 0003/0007. Preis: manche Teilnehmer bleiben ausschließlich manuell rekonzilierbar. |
| R4: Begrenzte Automation | Einzelne Überlastung lässt andere Arbeit und den Kontrollbetrieb überleben. | S04–06, S10, S15 | Aktivierungsgrenzen um globale Aufnahme-, Speicher- und Queue-Budgets ergänzen; Fairness zwischen Scopes. Preis: explizite Zurückweisung/Wartezeit statt scheinbar unendlicher Kapazität. |
| R5: Wiederherstellbare Evidenzmenge | Betrieb kann aus unabhängigen Kopien wieder aufgebaut werden, ohne Vergangenheit mit Gegenwart zu verwechseln. | S09, S11–13, S17 | DB bzw. Events, Idempotenzdaten, benötigte immutable Inhalte, Konfigurationsrevisionen und Recovery-Anleitung gemeinsam inventarisieren; separate Ausfalldomäne. Vault-Secrets nicht in ein allgemeines Backup-Paket kopieren. Preis: Retention-, Schlüssel- und Restore-Verantwortung. |
| R6: Portables, überprüfbares Arbeitspaket | Ein anderer Harness oder Mensch versteht Zweck, Empfänger, Inputs, Ergebnis und offene Fragen ohne Terminal-Scrollback. | S07–09, S14, S18 | Empfänger und Ausführungsrevision, sichere Input-Referenzen, Akzeptanzkriterien und getrennte Verifikation persistieren. Keine geheime zweite Kontextquelle; aktuelle Policy bleibt maßgeblich. Preis: Schema-/Versionspflege, aber kein allgemeiner Workflow- oder Message-Store. |

Wichtige Grenzen:

- R2 bleibt von einem funktionierenden Rechner und lesbaren Daten abhängig; erst R5 adressiert Geräteverlust.
- R1 ohne R4 kann sicher, aber dauerhaft handlungsunfähig werden.
- R3 kann externe Nicht-Idempotenz nicht wegabstrahieren.
- R5 repariert keinen bereits abgeflossenen geheimen Inhalt.
- R6 erhält die Arbeitsbeschreibung, nicht automatisch die Kompetenz oder Qualität eines Ersatzmodells.
- Keiner der Kandidaten macht S16 im heutigen Same-User-Vertrauensmodell sicher.

## 6. Sequenzen statt isolierter Fehler

### A — S01 → S02 → S06: Übergabe unklar, Runtime unsichtbar, Mensch abwesend

Nur Retry-Logik würde möglicherweise doppelt ausführen. Nur konservative Leases würden die Arbeit unbegrenzt festhalten. **R1 + R2 + R4** lassen die vorhandene Arbeit blockiert und geschützt, stoppen weitere Überlastung und erhalten den Diagnosepfad. Fortschritt des betroffenen Tasks bleibt bewusst ausgesetzt. Das ist kontrollierte Degradation, nicht automatische Selbstheilung.

### B — S10 → S11 → S12: SSD voll, Gerät verloren, alter Snapshot verfügbar

Ein lokaler Snapshot hilft nach Geräteverlust nicht. Ein externer DB-Snapshot reicht außerdem nicht, wenn Referenzinhalte fehlen. Und selbst ein vollständiges Restore kennt später ausgeführte Außenwirkungen nicht. Erst **R5 + R3 + R2** erlauben Rekonstruktion mit zunächst gesperrtem Dispatch und unabhängiger Effektprüfung.

### C — S07 → S09 → S08: Modell ersetzt, Kontext geändert, Ergebnis plausibel falsch

Ein Retry mit anderem Modell und heutiger `AGENTS.md` wäre ein anderer Lauf unter gleichem Etikett. **R6** erhält die alte Intention, zeichnet die neue Ausführungsentscheidung auf und trennt Abschluss von Verifikation. Bestehende externe Freigaben müssen dabei erneut auf ihre Gültigkeit geprüft werden.

### D — S14 → S18 → S05: Cutover missverstanden, beide Dispatcher aktiv, Nachholwelle

Datenbanklocks allein verhindern keine semantischen Doppelaufträge von zwei Systemen. **R3 + R4 + R6** benötigen einen identifizierbaren Mutationsinhaber, stabile Schedule-/Operation-Identitäten und einen eindeutigen Cutover-Bericht. Ein zweiter Daemon muss vor Dispatch scheitern; der Legacy-Writer darf nicht bloß „hoffentlich inaktiv“ sein.

## 7. Gegenproben auf die Kandidaten

Weitere Angriffe nach ihrer Formulierung; bewusst noch keine empirische Validation:

| Gegenprobe | Was hält? Was bleibt offen? |
|---|---|
| Neuer Harness kann keine stabile Session-ID liefern. | R1 verlangt nicht zwingend eine bestimmte ID-Form, wohl aber hinreichende Identitätsevidenz. Ohne alternatives Korrelationsprotokoll bleibt nur manuelle Bestätigung; automatische Recovery ist dann ausdrücklich nicht verfügbar. |
| Lesezugriff funktioniert, aber Events referenzieren inzwischen gelöschte Inhalte. | R5 ist widerlegt, wenn „Backup vollständig“ nur DB-Vollständigkeit meint. Das Manifest muss referenzierte Inhalte und zulässige Aufbewahrung abdecken; ein Hash allein stellt Inhalt nicht wieder her. |
| Operator widerruft eine Freigabe, während ein Plugin offline ist; später kommt dessen Antwort. | R3 braucht Dispatch-Preconditions und dokumentierte Grenze des Widerrufs. Ein bereits angenommener Effekt lässt sich nicht durch Statusänderung rückgängig machen. Späte Evidenz wird erfasst, nicht unterdrückt. |
| Ein Scope erzeugt dauernd korrekt autorisierte, kleine Tasks. | Frame-Limits und `max_sessions` reichen nicht. R4 braucht faire, begrenzte Aufnahme auch für formal gültige Last, sonst verhungern andere Scopes. |
| Zwei unabhängige Backups sind mit einem verlorenen Schlüssel verschlüsselt. | R5 scheitert dennoch. Wiederherstellbarkeit umfasst den freigegebenen Schlüssel-Recovery-Prozess; Schlüsselmaterial gehört nicht ins Analysedokument oder Task-Log. |
| Unternehmen verlangt harte Mandantentrennung. | Die Kandidaten verbessern Betrieb, lösen aber S16 nicht. Das ist ein neues Architekturmandat, kein weiteres Plugin-Konfigurationsfeld. |

Die Gegenproben erzwingen bereits Präzisierungen bei Retention, globaler Fairness und Widerruf. **Es gibt noch keine Evidenz, dass zusätzliche Stressoren keine neuen Strukturen mehr verlangen.**

## 8. Falsifizierbare Experimente

Noch nicht ausgeführt. Alle Tests zunächst mit synthetischen Daten, Fake-Teilnehmern und isolierten Fixture-Instanzen. Keine Produktionspanes stoppen, keine Live-DB restaurieren, keine Worktrees anlegen. Echte Runtime-/Geräte-/Backup-Drills erst als separat freigegebene Tasks.

| Experiment | Aufbau / Angriff | Bestehenskriterium |
|---|---|---|
| E01: Unsicherheit ist nicht Tod | Disconnected Session mit Lease; Fake-Adapter liefert nacheinander `Unavailable`, `Degraded`, unbekannte Identität und autoritativ aber `session_alive=false`. | Keine automatische Promotion, keine Lease-Freigabe aus diesen Antworten, kein zweiter Start im Workspace. Der offene Evidenzbedarf ist lesbar. F01/F02 lassen gegen den heutigen Arbeitsstand Gegenbeispiele erwarten. |
| E02: Jede Crash-Grenze der Zustellung | Fault Injection vor Journal-Commit, nach Commit, nach Writer-Effekt, vor Outcome-Commit; Fixture wieder öffnen. | Writer nie vor erfolgreichem Intent-Commit; nach möglicherweise erfolgter Zustellung kein weiterer automatischer Writer-Aufruf. Resume braucht eigene persistierte Autorisierung; keine pauschale neue Außenwirkungsfreigabe. |
| E03: Restore verliert spätere Wahrheit | Snapshot vor Dispatch; Fake-Teilnehmer führt danach Operation aus; alten Snapshot in Fixture restaurieren. | Kein automatischer Doppel-Effekt trotz im Snapshot fehlendem Versuch. Recovery erkennt die Evidenzgrenze und verlangt Rekonziliation bzw. manuelle Entscheidung. |
| E04: Ausfall- und Lastgrenzen | Fake-Plugin hängt/floodet; anderer Scope liefert gültige Arbeit; langsamer Subscriber blockiert; Workload übersteigt deklarierte Kapazität. | Deadline-, Byte-, Queue- und Speichergrenzen werden eingehalten; anderer Scope und Kontrollpfad erhalten messbaren Fortschritt. Konkrete Grenzwerte vor dem Test deklarieren, nicht nachträglich an Messwerte anpassen. |
| E05: Semantischer Roundtrip | Einen vollständigen Task-Lifecycle samt Freigabe und Effektabsicht aufzeichnen; nur abgeleitete Tabellen in Fixture entfernen, Replay mit alten Payload-Versionen und offline geschalteten Plugins. | Gleicher fachlicher Zustand und Cursor; exakt null Plugin-/Timer-/Prozessaufrufe. Fehlender Inhalt oder unbekannte Version stoppt an konkreter Position. Heute noch kein End-to-End-Feature. |
| E06: Wiederaufbau ohne Originalgerät | In isolierter Umgebung ausschließlich das genehmigte Recovery-Set verwenden; Originalpfade, Runtime und mutable Quellen nicht verfügbar machen. | Tasks und benötigte Evidenz vollständig erklärbar; Secrets nicht im Paket; fehlende Inhalte ausdrücklich benannt; RPO/RTO gemessen. Keine automatische Wiederholung möglicherweise erfolgter Effekte. |
| E07: Zeit und Mensch fallen aus | Deterministische Uhr um Sleep, DST und Rückwärtssprung bewegen; alle Worker auf Permission blockieren, keine Operatorantwort simulieren. | Genau die vorher spezifizierten Occurrences, keine unbeschränkte Nachholwelle, keine automatische Zustimmung; Ressourcen bleiben innerhalb deklarierter Budgets, read-only Diagnose bleibt möglich. |
| E08: Übergang und Intention | Legacy-Schema synthetisch nachbauen; Cutover unterbrechen; Task-Zielagent und Kontext während Wartezeit ändern. | Kein zweiter aktiver Writer, keine stille Schema-Adoption oder neue leere DB; ursprüngliche Intention bleibt sichtbar, aktuelle Policy kann alte Ausführung verweigern. |

Übergreifende Messgrößen: Anzahl unbeabsichtigter Writer-/Effekt-Aufrufe, Leases ohne Endevidenz freigegeben, unerklärbare Zustandsänderungen, Zeit bis lesbarer Diagnose, Ressourcenverbrauch bei Überlast, Zahl und Alter manueller Entscheidungen. Für Sicherheitsverletzungen ist das Ziel jeweils **null**; für Latenz, Kosten, RPO und RTO fehlen noch vereinbarte Zielwerte.

## 9. Empfohlene nächste Iteration — noch nicht beschlossen

### Zuerst, vor weiterer automatischer Runtime-Steuerung

1. **E01 als adversariales Recovery-Review:** F01/F02 gegen die aktuellen Dateien erneut bestätigen und den Evidenzvertrag festlegen. Keine Ausweitung der Automation auf unsichere Recovery bauen.
2. **Eine geltende Liefergrenze festhalten:** Welche Zusagen aus ADR 0003/0007 müssen vor unbeaufsichtigtem Betrieb umgesetzt sein? Mutable Bibliothekstabellen nicht als bereits replaybare Projektionen bezeichnen. Den bestehenden Backlog ergänzen, keinen zweiten Lieferplan auf Dauer pflegen.
3. **E02/E03 gemeinsam spezifizieren:** Restart, Resume und Restore müssen dieselbe Unterscheidung zwischen Zustellversuch, Außenwirkung und Autorisierung tragen.

### Danach, vor unbeaufsichtigtem Betrieb

4. Minimalen Kontrollbetrieb und begrenzte Last mit E04/E07 nachweisen.
5. Cutover des Prototyps und vollständige Recovery-Menge mit E06/E08 genehmigen und üben.
6. Replay- und Freigabeevidenz entsprechend der explizit vereinbarten Liefergrenze mit E05 beweisen.

### Nicht aus dieser Analyse ableiten

- Keine Empfehlung für Kubernetes, Multi-Node-HA oder eine neue externe Datenbank.
- Keine automatische Lockerung von Lease-, Freigabe- oder Schreibgrenzen.
- Keine Message-/Thread-Domäne oder allgemeine Workflow-Engine für v1.
- Keine pauschale Umsetzung sämtlicher Stressor-Gegenmaßnahmen. S16 kann durch eine ehrliche Einsatzgrenze adressiert werden; andere Risiken brauchen bewusste Akzeptanz statt sofortigen Ausbau.

### Drei Fragen an den Owner

1. **Was ist die minimale nützliche Factory während deiner Abwesenheit?** Reicht es, Arbeit sicher zu halten und spätere Diagnose zu erlauben, oder müssen bestimmte interne, reversible Arbeiten weiterlaufen?
2. **Wie viel Datenverlust und Stillstand nach Geräteverlust sind akzeptabel?** Daraus folgen RPO, RTO, Recovery-Umfang und die nötige getrennte Ausfalldomäne.
3. **Bleibt v1 ein vertrauenswürdiges Single-User-System?** Oder sollen fremde Plugins, Kunden-Repositories oder mehrere voneinander getrennte Betreiber hinein? Letzteres ändert die Sicherheitsarchitektur grundsätzlich.

## 10. Quellen und Einordnung

Interne Fundstellen; keine externe Primärliteraturrecherche zu Residuality Theory in dieser Iteration. Die Begrifflichkeit und der Ablauf oben sind eine praktische Annäherung, keine Behauptung einer zertifizierten oder vollständigen Methode.

- [Company-Design](../../../.specs/design.md), insbesondere §§2–6: Primitives, Schreibgrenze, Delivery, Recovery, kooperative Trust-Grenze.
- [Implementierungsbacklog](implementation-backlog.md), insbesondere Slices 5–13: Liefergrenzen, offene Übergänge und Akzeptanzkriterien.
- [ADR 0003](adr/0003-logical-api-and-event-sourcing-v1.md): kanonische Events, Replay, Idempotenz, Sagas, sichere Referenzen.
- [ADR 0007](adr/0007-supervised-plugin-process-protocol-v1.md): Plugin-Supervision, Scope-Aktivierung, Backpressure, unbekannter Outcome.
- [ADR 0010](adr/0010-design-baseline-reconciliation.md): Zielzustand versus v1, Terminologie und Geltungsgrenzen.
- [ADR 0014](adr/0014-daemon-owns-all-mutations.md): ein Mutator und read-only Offline-Diagnose als benannte Ausnahme.
- [ADR 0016](adr/0016-slice-3-registry-projection-and-reconcile.md): deklarierte Registry, beobachtete Pfade, Report vor Apply.
- [ADR 0019](adr/0019-restore-and-what-a-recovered-database-may-claim.md): konservative Restore-Aussagen; Datenbankbackup ist kein Instanzbackup.
- [Task-Store-Migrationsplan](task-store-migration-plan.md): laufender Prototyp und dokumentierte Schema-Kollision; in dieser Analyse nicht erneut live vermessen.
- [Store-Schema](../crates/factory-store/src/schema.rs), [Zustellung/Resume](../crates/factory-task/src/deliver.rs), [Assignment](../crates/factory-task/src/assign.rs), [Adapter](../crates/factory-adapter/src/lib.rs), [Recovery-Evidenz](../crates/factory-recovery/src/evidence.rs), [Reconnect](../crates/factory-recovery/src/reconnect.rs): konkrete statische Implementierungsbefunde.

**Arbeitshypothese:** Die wichtigste Verstärkung ist nicht, dass Factory möglichst viele Fehler automatisch repariert. Sie ist, dass Factory bei schrumpfender Evidenz **seine Befugnisse und Zusagen kontrolliert verkleinert**, während Arbeit, Kontext und Entscheidungen für eine sichere Fortsetzung erhalten bleiben.
