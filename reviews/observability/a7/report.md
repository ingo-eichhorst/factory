# A7: Beobachtung, Zeit und verbleibende Fähigkeiten

Run: `9b647544-a648-4f98-9907-f9db39fafda6`  
Bereich: **S201–S275**, ausdrücklich Folgephase nach A6.

## Ergebnis und Aussagegrenze

Alle 75 neutralen Szenarien haben genau einen Eintrag in `cases.json`. Darin stehen 171 bedingte Zweige. `residues.csv` beschreibt 42 lokal verwendete Kandidaten. Diese Zahlen beschreiben nur diese Abgabe: **keine Zielzahl, keine globale Modulentscheidung und kein empirischer Nachweis**.

Ein Residue bezeichnet hier einen konkret weiter nutzbaren Gegenstand oder eine Fähigkeit. Es ist nicht einfach der Fehler, der gewünschte Ausweg oder eine allgemeine Sicherheitsregel. Jeder Kandidat nennt Nutzer, Erhaltungsbedingungen, Ausfallgrenze, begrenzte Verantwortung, erforderliche Architekturarbeit und einen unterscheidenden Test. Eine vorgeschlagene Struktur ist ausdrücklich **nicht als heute implementiert** anzusehen. Auch ein plausibler Entwurf kann unter seinen Prüfbedingungen scheitern.

`kind` bezieht sich auf die im Zweig benannte Fähigkeit, nicht automatisch auf ganz Factory. Ein Halt unerlaubter Datenweitergabe kann mit erlaubter Arbeit zusammenfallen. Ein Abschluss kann eine wahre Ablehnung, eine Empfangsquittung oder einen beendeten Teilvorgang bezeichnen; er beweist nicht zugleich Geschäftserfolg, Prozessende und Ressourcenfreigabe. `teilweise` benennt ausdrücklich verlorene Eigenschaften. Leere Residue-Listen bei Verlust oder Ungewissheit werden nicht mit hypothetischen Kopien aufgefüllt.

Die Zweige sind unterscheidbare Bedingungen, keine vollständigen oder gegenseitig ausschließenden Zustandsautomaten. Sie beweisen auch keine erschöpfende Erfassung aller möglichen Verläufe. Gegenbedingungen zum wörtlichen Schadenseintritt sind als solche markiert, etwa eine rechtzeitige Teilnehmer-Sperre gegenüber dem zu späten Cancel in S234.

## Herkunft und eingefrorene Originale

Vollständig gelesen wurden:

- `../A7-PROTOCOL.md` und das ursprüngliche `../REVIEW-PROTOCOL.md`;
- eigene `scenarios.csv`, `states.csv`, `trajectories.csv`, `coverage.csv`, `report.md`;
- `../../docs/residuality/a6/state-qualifications.csv`;
- `../../docs/residuality/a6/supplemental-branches.csv`;
- `../skeptic/cross-review.md`.

Keine neuen Peer-A7-Ergebnisse wurden gelesen. Die ergänzenden A6-Zweige BR01–BR06 betreffen andere Szenariobereiche; sie werden nicht als eigene historische Zustände importiert. Ihre methodische Mahnung gilt auch hier: gewöhnlicher Fehler, bezahlbarer Aufwand oder ein endlicher Rückstau brauchen keine Rückkopplung.

`source_states` übernimmt ausschließlich die historische eigene Zuordnung. S218, S224, S230 und S274 behalten leere historische Listen; neue Formulierungen stehen ausschließlich in den Zweigen. Die jeweilige ursprüngliche Trajektorie heißt `OBT` plus Szenarionummer. Neue historische OB-IDs wurden nicht erfunden.

A7 hat keine Produktimplementierung geändert oder neu empirisch geprüft. Aussagen zu Quellfunktionen stammen aus dem historischen eigenen Evidenzverzeichnis beziehungsweise der ausdrücklich gelesenen skeptischen Nachprüfung. Sie bleiben diesen Funktionen und Voraussetzungen zugeschrieben, nicht einem vermeintlich vollständig ausgelieferten Daemon, Saga- oder Replay-System.

Die vor Beginn aufgenommenen SHA-256-Werte stimmen bei der abschließenden Prüfung unverändert:

| Original | SHA-256 |
| --- | --- |
| `scenarios.csv` | `3d1058c8ff0d717207a8b4f250751359adbe1bccda8787e971444151a3f05222` |
| `states.csv` | `87a6bcaf0bee88adf33c02ad808d2bdeb36457105530af2b6dbb5b9a1b42c282` |
| `trajectories.csv` | `d497eaf2d21712dcf9a50a2d0a90b63953a99b26b0a027144bd1b16154c1e0f6` |
| `coverage.csv` | `904609ea92f240f851ced26c8526cdc9fa729416328cbe7aba2bd9448ccf9758` |
| `report.md` | `e6ac9b9ddf5ca21f3055433682d9fda9da2844a44132694d6889d6843ac2a6d6` |
| `AGENTS.md` | `37fb2b6347a7908a46c98172520784d7781e8d939e594a8f3d5a1ace25d77870` |

## Zustandsmechanismen und übernommene A6-Korrekturen

### Beobachtung ist nicht Ausführung

S201/S204 verlieren bei Stromausfall aktuelle Ausführung, ohne daraus zerstörte Medien abzuleiten. S257 kann dagegen allein den Monitoringrouter verlieren, während Geschäft und lokaler Bestand weiter nutzbar sind. S207 verliert mit dem einzigen Monitor die Fähigkeit zur aktuellen Fernaussage. Fehlende Evidenz darf nicht als Grün fortgeschrieben werden.

Damit wird die pauschale Verlustformulierung des alten OB02 eingeschränkt, gemäß QF01/CF01. Getrennt bleiben Hostausführung, Geschäftserreichbarkeit, Beobachterfrische, Bedienzugang und menschlicher Empfang. S205 nimmt accountgesperrte Backups nicht als gelöscht an. S206 erhält menschlichen Empfang nur in einem ausdrücklich anderen Entwurf mit vorher vorhandener außerregionaler berechtigter Vertretung; der einzige unerreichbare Operator wird nicht still ersetzt.

S208 unterscheidet Versandannahme von einer echten Menschenquittung. S209 ist ein politisch beziehungsweise technisch gehaltener Mute, kein Attraktor aus zehn Jahren Dauer. S212 enthält sowohl legitimes Warten mit tragbarem Alarmaufwand als auch eine bedingte Aufmerksamkeitsschleife: falsches Kriterium → Lärm → Verwerfung → ausbleibende Korrektur → weiteres Rauschen. Ein endliches Alarmobjekt trägt ausdrücklich **nicht sämtliche offenen Vorfälle**. Das übernimmt QF11/CF12.

### Frische, richtige Zuordnung und Gesundheit sind verschiedene Beweise

Cache, Rückstau und Klon brauchen getrennte Annahmen zu Quellinkarnation, Sequenz, Alter und Aufnahmebudget. Ein vollständig kopierter Identitätsnachweis liefert in S215 keine magische Unterscheidung. Schlüsselüberlappung in S270 hilft nur innerhalb ihres vorher autorisierten Fensters. Nach dessen Ablauf ist unbekannt beziehungsweise gesperrt ehrlicher als Altkeys unbegrenzt zu akzeptieren.

Eine frische Antwort eines freien Threads beweist in S210 keinen Kernelcommit. In S211 können bestätigte alte Pages lesbar bleiben, während neue Writes scheitern. In S217 entscheidet zusätzlich die Lage des Traceexports: vor Commit wartet die neue Mutation; nach Commit kann die Mutation bereits angewandt sein, obwohl der Caller noch wartet. Ein Exporttimeout darf daraus keinen Rollback machen.

### Endliche Last, äußere Dauerlast und Rückkopplung

Ein endlicher Retrybaum, eine Wiederanlaufkohorte oder eine Milliarde alte Vorkommen sind zunächst endlich. Strikter Notfallvorrang unter anhaltendem Input und thermisch begrenzter Dienst unter Dauerlast sind äußerlich erhaltene Regime. Ein fortlaufender Enkelprozess kann zwei Tage lang schreiben, ohne sich selbst zu reproduzieren.

Die stärkeren Hypothesen nennen dagegen tatsächliche Kanten:

- S237/S238/S253: Rückstau → Timeouts/Quotenfehler → erneut erlaubte Versuche beziehungsweise synchronisierte Wellen → größerer Rückstau.
- S240: Speicherdruck → zerstörter Warmfortschritt → erneutes Laden → weiterer Speicherdruck. Ohne erneuertes Startbudget endet auch dieser Prozess endlich.
- S242: abgebrochener Aufbau → neuer Kaltversuch → Konkurrenz und längerer Aufbau → erneuter Abbruch. Ein einzelner langsamer Erstaufruf reicht dafür nicht.
- S255: Wärme → geringere Rate → Queue/Timeouts → zusätzliche Retryarbeit → anhaltende Wärme.
- S267/S269/S275: kontrollierte Persistenz → kompromittierte Ausführung und Evidenz → darauf gestützte Recoveryentscheidung → erneute Aktivierung der Persistenz. Eine einmalige signierte Exfiltration ist stattdessen historischer Verlust.

Gemäß QF09/CF10 wird zuerst der ursprüngliche Fehler **bei demselben deklarierten Grundbedarf und derselben Policy** entfernt; unterschiedliche Rückstaustände werden verglichen. Null-Neuzugänge sind ein eigener Versuch, kein Ersatz für diesen Vergleich. Es wurde kein solcher Versuch ausgeführt.

S254 übernimmt QF10/CF11: endliche Scopezahl und vererbte Nichtwiederholung begrenzen Tiefe, nicht Breite. Weitere Wachstumsreproduktion benötigt explizit frische Wurzelautorität oder andere fortgesetzte Erzeuger. Die offene Wachstumsalternative behauptet keinen stabilen Attraktor.

### Besitz, Wirkung und Wissen nicht gemeinsam freigeben

QF03/QF04/CF03/CF04 werden in S203/S235/S245 konkret: Taskfreigabe, Workspacebesitz, OS-Schreibautorität und Teilnehmerausgang sind getrennte Prädikate.

Der historische Restore-/Disconnected-Pfad hält Leases. Das skeptisch geprüfte `reconnect_after_herdr_or_machine_restart` kann dagegen bei fehlender positiver Reconnection einen Ersatzdatensatz unter angenommener Abwesenheit erzeugen. Dieser Datensatz startet noch keinen Prozess. Tatsächliche zwei Writer benötigen zusätzlich Ersatzstart und überlebende alte Schreibrechte. Kein globaler positiver Todesbeweis wird aus einem Helper abgeleitet.

S256 übernimmt ausschließlich die belegte lokale Commit-vor-Writer-Form. Der historische Writer ist eine manuelle Übergabe, kein nachgewiesener Mail- oder Rechnungsteilnehmer. S260 bleibt getrennt: die betrachteten direkten `stop`/`cancel`-Funktionen propagieren Commitfehler vor Erfolg. Ein lügender Wrapper ist eine Gegenbedingung, keine beobachtete Eigenschaft. Eine volatile Notsteuerung kann nützlich sein, darf aber keinen gespeicherten Neustartschutz behaupten. Das berücksichtigt QF05/QF17 und CF05/CF19.

### Informationsverlust und historischer Schaden sind eigenschaftsbezogen

S231 unterscheidet nur lokal fehlende Historie, einen wirklich noch vorhandenen Teilnehmerbeleg und den Verlust der letzten rechtmäßig nutzbaren Zuordnung. Eine späte Antwort sendet nicht selbst nochmals. S233 stellt eine rechtmäßig gelöschte Beziehung nicht für bequemeres Recovery wieder her. Inhaltsarme Ablehnung erhält eine Handlerfähigkeit, nicht gelöschte Identität oder alte Nutzlast.

S250 nennt auch die harte Grenze: Wer die letzte geprüfte Quelle vor einem brauchbaren Ersatz löscht, kann ohne weitere Quelle keinen verlorenen Stand restaurieren. S271 inventarisiert Schlüssel, Decoder, Hardware und Zeitraum. S272 kann Geheimhaltung einer schon kopierten und entschlüsselten Exportgeschichte verlieren, ohne damit heutige Ausführung zu verlieren. Das folgt QF12/CF13.

S226/S234/S251 trennen historische Annahmen von später behebbaren Folgen gemäß QF13/CF14. Ein finanziell korrigierbares Duplikat bleibt eine frühere zweite Annahme. Ein zu später Cancel kann künftige Arbeit stoppen, aber eine bereits gesendete Nachricht nicht ungeschehen machen.

## Residue-Splits, Wiederverwendung und Modulgrenzen

`module_hint` ist eine fachliche Verantwortung, **kein Auftrag für einen eigenen neuen Dienst pro Zeile**. Gemeinsame Implementierung ist möglich, wenn die unterschiedlichen Verträge sichtbar bleiben.

- **Bestand:** OBR001 wird breit wiederverwendet, aber immer nur für zuvor bestätigte, lesbare Datensätze bei ausdrücklich intaktem Medium und Decoder. Er bedeutet weder vollständige aktuelle Geschichte noch rekonstruierbaren Klartext, richtige Arbeitsergebnisse oder aktuelle Bedienbarkeit eines stromlosen Hosts.
- **Beobachtung:** OBR002 ist begrenzte unabhängige Ausfallevidenz, OBR003 menschliche Quittung eines Vorfalls, OBR008 begrenzter offener Vorfallsbestand. OBR004 bindet Frische an eine Inkarnation, OBR005 bindet Gesundheit an den geprüften Dienstpfad. Keiner ersetzt die anderen.
- **Diagnosekosten:** OBR009 erhält zugelassene Metrikreihen, OBR010 einen begrenzten asynchronen Tracerest. Beide brauchen echte Ressourcenbegrenzung, aber Labelkardinalität und ein blockierender Commitexport sind nicht derselbe Gegenstand.
- **Zeit und Aussagen:** OBR012 ist der unverjüngte Zeitvertrag, OBR013 der Aufnahmenenner, OBR014 die beschränkte Stichprobenaussage. OBR015 erhält vorhandene Prüfbefunde mit Fehlstellen. Eine bessere Stichprobe liefert keinen fehlenden Vetoprüfer und reclaimt keinen Slot.
- **Wirkung:** OBR017 ist lokales Intent-/Versuchswissen, OBR018 das tatsächliche Teilnehmerannahmebuch. OBR019 hält Kontrolle bedienbar, OBR020 entzieht Schreibautorität, OBR021 hält eine konkrete unklare Reservierung. Kein Timeout oder lokales Journal beweist allein die anderen Grenzen.
- **Kapazität:** OBR016 reserviert echte Restkapazität; OBR022 zählt Versuche; OBR023 begrenzt Geld; OBR024 budgetiert Kaltstart; OBR026 sichert einen Dienstanteil. Zeitstreuung erzeugt keines dieser Budgets. OBR033 ergänzt Baumgesamtaufnahme statt nur endlicher Kettentiefe.
- **Große Daten und Recovery:** OBR027 erhält gezielten Registryzugriff, OBR028 begrenzte Frameannahme, OBR029 ein begrenztes Cursorfenster. OBR030 benötigt einen wirklich vorhandenen geprüften Replayeinstieg, OBR031 einen tatsächlich passenden Stufenplan. OBR032 erhält legitim disponierte Scheduleintervalle, nicht eine Milliarde vorgespiegelte Erfolge.
- **Autorität:** OBR035 wird für Empfängertausch, importierte IDs und Dokument-/Log-/Artefaktinjektion wiederverwendet, weil immer dieselbe unabhängige aktionsgebundene Freigabe maßgeblich ist. OBR036 ist dagegen die Scope-Credential-Grenze und OBR034 vorab eng erteilte Offline-Notautorität.
- **Vertrauen und Daten:** OBR037 erhält ausführbare Archivbytes samt Prüfanker, nicht automatisch harmlosen Code. OBR038 ist ein anders verwalteter begrenzter Nur-Lese-Zeuge, kein perfektes Kompromissorakel. OBR039 erhält einen konkret unerreichbaren, noch nicht offengelegten Datenbereich. OBR041 behandelt Timing separat: OS-Leseverbot ist keine Zeitkanalabschirmung.
- **Bewusster Nachsplit:** OBR042 in S233 ist zulässige, inhaltsarme Ablehnung unzugeordneter sensibler Rückläufer. Eine reine Größenprüfung OBR028 könnte eine kleine sensible Payload trotzdem falsch routen. Deshalb wurde die anfängliche Wiederverwendung in der Eigenprüfung getrennt. Ebenso erhielt der Such-Pflichthalt S241 kein Approvalresidue, wenn lediglich Suchinformation und nicht Erlaubnis fehlt. S212 trennt fehlende Freigaben von anderen legitimen externen Voraussetzungen; Geldbudget in S239 wird nicht automatisch zum gemeinsamen Versuchszähler, Teilnehmer-Cancelstatus in S234 nicht zum unabhängigen Kontrollpfad.

Diese Trennungen übernehmen insbesondere QF06/QF07/QF15: eine benannte Schutzregel ist kein bewiesenes Produktfeature, Ablehnung ist nicht automatisch nützlicher Abschluss, und unabhängige Evidenz muss Entscheidungen tatsächlich beeinflussen können.

## Die vier bisher offenen Fälle

| Fall | Fehlende entscheidende Annahme | Bedingte Alternativen und Grenze |
| --- | --- | --- |
| S218 | Gleiche Exposition, Instrumentierungsort und konkurrierende Zugriffsordnung | Debug kann die kritische Ordnung verändern; dann ist ein vergleichbares Timingdossier OBR011 nützlich. Weniger ausgeführte Versuche können Nullfehler aber ebenfalls erklären. Ohne unterscheidende Evidenz bleibt die Race-Diagnose ungeklärt. |
| S224 | Definition von Verfügbarkeit, Auswahlregel, Nichtabschlüsse, Stationarität und Konfidenz | Die erhaltenen Samples tragen eine enge Aussage OBR014. Unter zusätzlich unabhängigen stationären Bernoulliversuchen ergibt `1 - 0.05^(1/1000)` rund `0,002991`, nicht `0,00001`. Ein operatives Langzeitregime folgt daraus weiterhin nicht. |
| S230 | Collectortrigger, Deadlinephase, Jitter, Beobachtungsdauer und Auswahl | Periodische Allokation kann wiederkehrende Kollision äußerlich treiben. Zufall oder selektive Beobachtung bleiben andere Alternativen. Timingdossier und unveränderter Zeitvertrag helfen nur unter den ausdrücklich vorhandenen Messbedingungen. |
| S274 | Angreifersensoren, Kontrolle, Opfergrundrate und akzeptierte Fehlerrate | Tatsächlich über die Vorwissensbaseline erschlossene Aktivität ist bereits offengelegt. Ohne Modell bleibt Inferenz offen. Eine entworfene Timingabschirmung OBR041 ist nur für genau diese Sensoren und Kostenbedingungen ein Kandidat, kein allgemeiner Geheimhaltungsbeweis. |

Keiner dieser Fälle wurde durch neue historische IDs oder erfundene eindeutige Konvergenz „geschlossen“. Die zusätzlichen Zweige präzisieren Annahmen und Prüfbedarf, nicht einen Erfolgsscore; das übernimmt QF08/CF09.

## Unterscheidender Prüfbedarf

Nur Vorschläge für eine später gesondert autorisierte, isolierte Aufgabe mit synthetischen Daten:

1. **Abhängigkeitsmatrix:** Strom, Account, Region, Monitor und Mensch getrennt entziehen. Prüfen, welches genaue Wissen beziehungsweise welcher Empfang überlebt, nicht nur ob irgendwo eine Mail erzeugt wurde.
2. **Frische und Dienstpfade:** Cache, Klon, Rückstau und Rotation gegen originale Inkarnationen testen; Hostping, Read und bestätigte Testmutation separat blockieren.
3. **Alarmgrenzen:** Wartungsablauf über Neustart/Uhrsprung prüfen. Wiederholungsmeldungen und echte neue Vorfälle mischen; Quittieren darf offene Vorfälle nicht löschen. Überlauf muss Abdeckungslücken zeigen.
4. **Zeit und Statistik:** Serielle und überlappende Hops, Queuezeit, DNS/TLS, offene Hänger und geschlossene Lastgeneratoren vergleichen. Ein tausendfacher kleiner Sampletest prüft nicht das extreme Produktions-Tail.
5. **Rückkopplungen:** Initialfehler bei gleichem Grundbedarf und gleicher Policy entfernen, Rückstau variieren, erst danach Neuzugänge stoppen. Retry-, Lade-, Wärme- und Frischwurzelarbeit einzeln zählen.
6. **Wirkung und Kontrolle:** In inertem Teilnehmermodell lokale Intents, verlorene Antworten, Retentionsablauf, Cancel vor/nach Annahme und späte Enkelwrites kreuzen. Gegenstand des Nachweises ist Annahme beziehungsweise entzogene Autorität, nicht Antworttext.
7. **Summenressourcen und Restore:** Gesamtlabels, Frames, Subscriber und reale Serviceanteile begrenzen. Checkpoint gegen Vollreplay vergleichen; RTO getrennt messen. Stufenrestore an jeder Abbruchgrenze auf letzte gute Quelle und Peakplatz prüfen.
8. **Vertrauensgrenzen:** Inerte falsche Freigaben, Empfängertausch und Scope-Credentials am tatsächlichen Gate prüfen. Supply-chain-, OS- und Timingtests brauchen jeweils eigenes Angreifermodell. Nach legaler Löschung weder neue Beziehung noch sensible Logs aus Rückläufern erzeugen.

## Abgabeprüfung

UTF-8-CSV wurde mit Semikolonparser und vollständigem Header geprüft; JSON wurde vollständig geparst. Geprüft wurden genau S201–S275 je einmal, die vorgeschriebenen Schlüssel, erlaubte Typen und Status, nichtleere Begründungen und Prüfbedingungen, gültige historische Zustandsreferenzen, existierende und tatsächlich verwendete Residues sowie nichtleere Residue-Listen bei `bedingt`/`teilweise`. Alle vier historischen offenen Fälle bleiben ohne erfundene Quellzustände.

Zusätzlich wurden Wiederverwendungen nach Gegenstand und Vertragsgrenze geprüft; daraus entstanden die dokumentierten Korrekturen zu S233 und S241. Die Originalhashes blieben gleich. Geschrieben wurden ausschließlich `a7/residues.csv`, `a7/cases.json` und dieser Bericht. Materialentscheidungen und Fortschritt stehen im zugewiesenen Run.

Das sind **Konsistenz- und Analyseprüfungen**, keine Tests implementierter Schutzstrukturen, keine Livefehler, keine externen Aktionen und keine empirisch bestätigten Attraktoren.
