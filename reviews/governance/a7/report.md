# A7 Governance: bedingte Zustände und noch nutzbare Gegenstände

Run: `6ee00c45-6db4-4518-89f0-de751729c3b8`  
Bereich: **S101–S200**, Folgephase nach abgeschlossener Phase 1  
Stand: 2026-09-09

## Ergebnis und Leseregel

Alle 100 neutralen Karten sind einzeln bearbeitet. `cases.json` enthält 214 bedingte Zweige. `residues.csv` enthält 38 lokal definierte und tatsächlich verwendete Kandidaten. Diese Zahlen beschreiben nur diese Abgabe. Sie sind weder Zielzahlen noch eine endgültige globale Architekturanzahl. Insbesondere wurden alte Zahlen nicht als Auswahlmaßstab verwendet.

**Kein Kandidat wird als heute implementiert, tatsächlich überlebendes Produktfeature oder empirisch bestätigt ausgegeben.** Die Aussagen bedeuten: Wenn die benannten Gegenstände, Rechte und Grenzen bestehen und den jeweiligen Ausfall überstehen, kann der bezeichnete Nutzer diese konkrete Fähigkeit noch verwenden. Eine gewünschte Reparatur allein zählt nicht als Residue.

- `conditions` nennt die Voraussetzungen des Zweigs; die zugehörigen Voraussetzungen und Ausfallgrenzen im Katalog gehören dazu.
- `bedingt` behauptet nur die ausdrücklich begrenzte Erhaltung unter diesen Voraussetzungen, nicht den Erfolg des gesamten Szenarios.
- `teilweise` bezeichnet einen engeren nutzbaren Gegenstand oder Teilvertrag. Ein lesbarer alter Receipt ist beispielsweise kein noch aktives Fencing.
- `keines` bezieht sich auf die ausdrücklich genannte Fähigkeit. Fehlende Beweise, verlorene Frist oder offengelegtes Geheimnis bedeuten nicht automatisch Verlust aller Aufgaben.
- `ungeklaert` bleibt stehen, wenn die verfügbare Beschreibung selbst die betroffenen Gegenstände bzw. ihre Nutzbarkeit nicht bestimmt.
- Zweige sind keine vollständige Partition. Manche sind konkurrierende Bedingungen, andere ausdrücklich getrennte Eigenschaften desselben Vorfalls, etwa Exportlesbarkeit und offene externe Verpflichtung.
- `source_states` übernimmt unverändert die historische Herkunftsmenge. Diese IDs sind keine Behauptung, dass alle alten Zustände gleichzeitig auftreten. Neue Zustandsnamen erhalten hier keine erfundenen historischen IDs.

Geschrieben wurden ausschließlich `a7/residues.csv`, `a7/cases.json` und dieser Bericht. Keine Implementierung, Konfiguration, Agentenverwaltung, Livefehler, externe Wirkung oder Worktreeoperation wurde ausgeführt. Kein neues Peer-A7-Ergebnis wurde gelesen.

## Grundlage und eingefrorene Originale

Vollständig gelesen wurden:

- `../REVIEW-PROTOCOL.md` und das für diese Folgephase maßgebliche `../A7-PROTOCOL.md`;
- eigene `scenarios.csv`, `states.csv`, `trajectories.csv`, `coverage.csv` und `report.md`;
- `../../docs/residuality/a6/state-qualifications.csv`;
- `../../docs/residuality/a6/supplemental-branches.csv`;
- `../skeptic/cross-review.md`.

Die 48 historischen Zustände und 100 Ursprungstrajektorien wurden nicht verändert. Die fünf bisher offenen Karten behalten eine leere historische Herkunftsliste. Sourceaussagen sind **aus dem historischen Evidenzregister übernommen**, nicht in A7 als aktueller Checkout oder laufendes Produkt neu verifiziert. Das gilt insbesondere für `compile`, `complete_with_result`, `restore::reconcile`, mutable SQL-Tabellen und die Lineageprüfung. Designversprechen über Eventreplay, Plugins, Telemetrie oder Sagas werden nicht zu vorhandenem Verhalten aufgewertet.

Vor Bearbeitung und bei der Abschlussprüfung stimmten diese SHA-256-Werte bytegenau überein:

| Original | SHA-256 |
| --- | --- |
| `scenarios.csv` | `e955a81a3abfecc54451f9957f988a707b7062690da9d5f45890c7f30c0f4441` |
| `states.csv` | `85429e2ba0bb67c0cae981cd41db612f833a3f414094f251f03436ba634d3127` |
| `trajectories.csv` | `3d6fc66b6e22e2f085d824cdf0fe1f74414dcd4ebfdd436bcdf294d62b5554ad` |
| `coverage.csv` | `0f2c51e1d35b30d471419dc96c476bcdb647f28c3e31f32d667f12efef96e0d0` |
| `report.md` | `7d7cef16ef1c748cf6e5a734d19a746cd79787c3baaf75687f5cc93acc603c98` |

## Neu konkretisierte Zustandsmechanismen

### Ein gemeinsamer Irrtum ist noch keine Rückkopplung

S104 enthält entsprechend BR01 ausdrücklich die einmalige falsche Doppelbestätigung als `transient`. Erst Übernahme als neue Autorität und wiederholte Bestätigung schließen den Kreis. S102, S103, S107, S184 und S195 benennen ebenfalls diese tatsächlichen Kanten. Bei Test- und Organisationsfällen verläuft der andere Kreis über Bewertung, Belohnung und Entzug unabhängiger Prüfung, etwa S110, S157, S167 und S200.

Ein lesbares Eingabepaket oder Bewertungsprotokoll kann in einem falschen Konsens erhalten bleiben. Es beweist weder die Wahrheit des Inhalts noch die politische Wirksamkeit einer Widerlegung. GVR006 setzt deshalb einen **konkreten** unabhängigen Sachbefund und dessen Einfluss auf Annahme voraus. S184 enthält daneben ausdrücklich den Zweig ohne verfügbare entscheidende Information: keine zusätzliche perfekte Prüfinstanz wird erfunden.

### Endliche Last, äußere Dauerlast und neue Arbeit unterscheiden

S130 und S196 respektieren endliche Scopezahl und erhaltene Lineage. Ein riesiger endlicher Baum kann Ressourcen erschöpfen, ohne unendliche Delegationstiefe zu haben. Die Rückkopplungshypothese verlangt autorisierte frische Wurzeln oder andere fortgesetzte Produzenten und eine Reproduktion oberhalb des Abbaus. S108 ist dagegen das Problem fehlenden Endkriteriums bei richtigen Schritten; endliche Energie unterbricht die idealisierte endlose Arbeit.

Die vorgeschlagene Prüfung entfernt zuerst den auslösenden Fehler bei **gleicher deklarierter Basislast und Policy**. Danach wird getrennt ohne neue Ankünfte geprüft. Drainage nach Abschalten aller Nachfrage widerlegt nicht allein unterschiedliche Regime bei unverändert positiver Nachfrage. Kein solcher dynamischer Versuch wurde hier ausgeführt.

### Vier unabhängige Sperren statt eines allgemeinen Recoveryzustands

S165 und S193 trennen:

1. legitime Befugnis für eine neue Entscheidung;
2. Wissen über schon angenommene externe Wirkung;
3. tatsächliche Prozess-/Workspacebelegung;
4. erlaubte und finanzierbare weitere Arbeit.

Runtimewiederkehr löst weder Nachfolge noch Finanzierung. Ein verfügbares Passwort schafft keine legitime Person. Ein gespeichertes Cancel beweist kein Prozessende. Die konservative Wirkung von `restore::reconcile` darf nach QF04/CF04 nicht auf andere Recoverypfade übertragen werden. Eine lokale Lease entzieht keinem weiterlebenden Prozess seine OS-Schreibrechte.

S191 und S198 unterscheiden zudem **lokale Sendeabsicht**, **tatsächliche Teilnehmerannahme** und **unwiderruflich verlorene Eigenschaft**. Zwei lokale Absichten sind kein bewiesener Doppelvollzug. Ein tatsächlich doppelter Zahlungsvorgang kann finanziell ausgeglichen werden, ohne historisch nur einmal stattgefunden zu haben.

### Verlust ist auf eine bestimmte Eigenschaft begrenzt

- S113/S120/S121: gespeicherte Kopie und unberechtigtes tatsächliches Lesen sind nicht dasselbe. Nach Lesen ist Nie-Offenlegung verloren, nicht automatisch jede künftige Funktion.
- S133: verbotener Verarbeitungsort kann ohne neu belegten unberechtigten Klartextleser vorliegen. Die Korrekturidee aus BR06 wird auf die eigene Residenzkarte angewandt, ohne fremde Stressorobjekte zu übernehmen.
- S139/S194: erlaubte Herkunftskanten können bleiben, während notwendige Payload nicht rekonstruierbar ist. Ein Hash ersetzt keinen Inhalt.
- S148/S178: später brauchbare Bytes ersetzen nicht die ursprüngliche harte Frist.
- S183: lesbare Sprache und rekonstruierte historische Geschäftsabsicht sind getrennt.
- S188: gebrochene Authentizitätsannahme ist noch kein nachgewiesenes Fälschungs- oder Entschlüsselungsereignis.
- S129/S181/S199: kein heute bekannter Schlüssel-/Content-/Decoderweg ist schwächer als dauerhafter Verlust jeder Information.
- Nur S190 übernimmt die wörtlich vollständige Grenze einschließlich fehlender rekonstruktiver Spuren und Erinnerungen. Dort gibt es kein Residue für systemspezifische Kontinuität.

## Konkrete Splits und Wiederverwendungen

Die Modulhinweise benennen begrenzte fachliche Zuständigkeiten, **keine Aufforderung zu 38 neuen Diensten**. Kernmutation bleibt beim Daemon; tatsächliche OS-, Provider-, Transport- und Teilnehmergrenzen gehören in ihre ausdrücklich aktivierten Adapterverträge. Factory-eigene Dateiausgaben sind unter `.factory/` zu gestalten; dieser Review ist Agentenarbeit und keine Produktdateiausgabe.

| Trennung | Warum sie für die Fälle notwendig ist |
| --- | --- |
| GVR001 Bestandsakte / GVR037 Artefaktidentität / GVR002 abgenommenes Ergebnis | Ein Taskrecord ist kein vorhandenes File; eindeutige Bytes sind noch kein zweckspezifisch gutes Ergebnis. S105/S119/S149. |
| GVR003 Eingabepaket / GVR004 Rohmessung / GVR023 Kalibrierung | Erfasste Eingabe rekonstruiert keinen verdeckten Modellwechsel. Rohwerte erlauben nur bekannte Dimensionen. Kalibrierung trägt einen belegten Instrumentvergleich. S107/S109/S111/S166. |
| GVR006 Gegenbefund / GVR031 Belegverzeichnis | Ein tatsächlicher unabhängiger Sachtest kann eine konkrete falsche Abnahme stoppen. Eine lesbare Liste kann nur vorhandene und fehlende Evidenz zeigen; sie erzeugt keine Tests oder Zustimmung. S104/S184/S200. |
| GVR005 gehaltene Effekthülle / GVR010 Personenbeleg / GVR032 Befugnisordnung | Nichtfreigabe, historische persönliche Zustimmung und legitime heutige Zuständigkeit haben verschiedene Nutzer und Ausgänge. S123/S136/S161/S165. |
| GVR011 aktueller Widerruf / GVR021 Operationsregister / GVR038 Schreibepoche | Alter Grant, bereits angenommene Operation und unberechtigter paralleler Writer sind drei verschiedene Fragen. Deduplizierung derselben ID stoppt nicht jeden fremden Writer mit neuer ID. S127/S170/S175/S191/S198. |
| GVR012 vollständiger Recoverysatz / GVR016 Lauf- und Bausatz / GVR025 Recoverynachweis | Daten mit Schlüssel, ausführbare rechtmäßige Tools und tatsächlich gemessene Wiederherstellung dürfen nicht stellvertretend füreinander zählen. S140/S155/S178/S199. |
| GVR014 Verfügungsverzeichnis / GVR015 Herkunftskanten / GVR029 Bedeutungsschlüssel | Erlaubte Datenbehandlung, rekonstruierbare Herkunft und verstandene Geschäftssemantik sind andere Gegenstände. S131/S139/S183/S194. |
| GVR017 Kostenobligo / GVR018 Ausgabenreservierung | Bekannte Nutzung mit unbekannter Rechnung ist lesbar, aber keine bindende Kostenschranke. S141/S142/S146/S150. |
| GVR013 Arbeitsmenge / GVR019 Ressourcenanteile | Endliche Verpflichtungen sind nicht automatisch fair verteilt oder innerhalb einer RAMreserve ausführbar. S130/S143/S145/S171/S196. |
| GVR024 wirkungsfreie Offlineentwürfe / GVR028 Checkpoint | Ein Entwurf enthält keine heutige Außenvollmacht. Ein Checkpoint braucht einen tatsächlich ins Zeitfenster passenden nützlichen Fortschritt. S173/S174/S187. |
| GVR026 Empfängerexport / GVR036 Publikationspaket | Selbständige erlaubte Datennutzung nach Dienstende und Ausschluss von Companyobjekten vor Veröffentlichung prüfen verschiedene Vertragsgrenzen. S156/S179. |
| GVR008 OS-Datenbereich / GVR009 Startobjekt / GVR033 unabhängiger Tatsachenbeleg | Vertraulichkeitsgrenze, geprüfte ausführbare Bytes und Herkunft gegen Manipulation sind nicht ein allgemeines Sicherheitsmodul. S117–S128/S177/S188. |

Besonders wichtige Wiederverwendungsgrenze: GVR021 in S170 oder S179 kann **teilweise** als tatsächlich erhaltener konkreter Receipt weiter lesbar sein, selbst wenn der ursprüngliche Teilnehmer nicht mehr arbeitet. Es wird daraus weder fortgesetzte Deduplizierung noch exklusiver Writerbesitz abgeleitet. S175 und S198 benötigen für den zweiten Anspruch zusätzlich GVR038 am tatsächlichen Teilnehmerpfad.

GVR005 ist nur nutzbar, wenn der konkrete noch nicht angenommene Effekt wirklich vollständig vermittelt bleibt. Eine bloße Richtlinie, eine erfolglose Cancelnachricht oder ein allgemeiner Resilienzanspruch genügt nicht. Die Selbstprüfung entfernte deshalb die unpassende Wiederverwendung bei S200. Dort bleibt das Belegverzeichnis, kein erfundener konkreter Effektauftrag.

Weitere begrenzte Gegenstände bleiben bewusst eigenständig: inerte Diagnoseansicht (S126), unabhängige Startdiagnose (S180), Notfallstopspur trotz voller SSD (S192), bedingter Kompensationsauftrag samt Objektversion (S197) und separat validierter lokaler Bewegungscontroller (S176). Keine dieser Grenzen wird als heutiges Factoryfeature behauptet.

## Bisher offene Karten: jetzt bedingte Alternativen, keine erfundene Entscheidung

| Karte | Fehlende entscheidende Annahme | Ausgearbeitete Alternativen |
| --- | --- | --- |
| S125 | Tatsächlicher SQL-/Pfadsink, Objektauflösung und effektive Schreibrechte | Gebundene Daten und nachgewiesene unerreichbare Fremdgrenze; oder erreichbare unerlaubte Mutation mit noch unbekannten Zielobjekten. Zeichenfolgen allein sind kein Exploit. |
| S174 | Erlaubter Remote-Kern, Offlinefunktion, OS-Laufrecht und Frist | Verbundener Remoteclient; haltbare intermittente Entwurfsarbeit; oder nicht ausführbare autonome Fristwirkung während vollständiger Suspension. Ein Remote-Kern ist nur im ausdrücklich so bedingten Zweig vorhanden. |
| S175 | Partitionspolitik, erlaubte Unverfügbarkeit, externe Effektidentität und Writerbesitz | Fenced Seite hält bei fehlendem Recht; beide Seiten schreiben divergent; oder Garantieziel bleibt ohne Vertrag offen. Historische Klonakten sind keine wirkungsfreien Offlinepakete. |
| S185 | Gemeinsames Objekt, Wirksamkeitszeit, Jurisdiktion und Bedeutung von legitim | Zwei korrekt verschiedene Bereiche; derselbe strittige Effekt bleibt gehalten; oder Bedeutung der Anforderung bleibt offen. Identische Technik entscheidet keinen Rechtsvorrang. |
| S186 | Bindende Definitionen von Löschung, Beweis, Ausnahme und Vorrang | Wörtlich unvereinbare Vollpflichten haben kein gemeinsam erfüllendes Residue; eine tatsächlich erlaubte ausreichende Metadatenstruktur kann einen engeren Beweis tragen; ohne Auslegung bleibt Rechtskonformität offen. |

Alle fünf behalten `source_states: []`. Neue Formulierungen stehen ausschließlich in den Zweigen. Die Originale werden nicht nachträglich so umgeschrieben, als seien diese Alternativen früher schon indexiert gewesen.

## Gegenbeispiele und verbleibender Prüfbedarf

1. **Same UID ist keine Sandbox.** Der gesamte GVR008-Zweig fällt, wenn native Konfiguration, Installhook oder Keychainzugriff dieselbe vermeintlich getrennte Grenze erreichen. Ein isolierter Test muss genau die effektiven Rechte benutzen, nicht nur die freundliche API.
2. **Geprüfter Pfad ist kein geprüftes Startobjekt.** S122 verlangt Tauschversuch einschließlich Hooks und nachgeladener Abhängigkeiten. S159 lässt ein zufällig richtiges Ergebnis trotz falscher Reihenfolge zu; das zertifiziert keinen sicheren Updater.
3. **Unbekanntes Postpaid hat keine aus Tokens ableitbare Geldschranke.** S142 benutzt deshalb keine Ausgabenreservierung mit erfundenem Höchstpreis. Ein späterer verbindlicher Tarif wäre eine neue Bedingung.
4. **Ein Receiver kann gestorben sein, obwohl ein Receipt lebt.** Outcomeprüfung und aktive Sperre müssen mit zwei verschiedenen Versuchen geprüft werden: derselbe Operationsbezeichner erneut und ein neuer Bezeichner unter alter Schreibepoche.
5. **Ein fehlender Personenbeleg ist kein überlebender Personenbeleg.** In S157 bleibt bei Abweisung nur die gehaltene Effekthülle. Personenbelege werden nur referenziert, wenn tatsächlich vorhandene entsprechende Bindung vorausgesetzt ist.
6. **Kleine Energiezeit kann jeden Fortschritt verhindern.** GVR028 braucht Boot, sinnvollen Schritt und Commit im Fenster. S187s engere Aktenlesbarkeit gilt nur, wenn gerade diese Inspektion noch hineinpasst; andernfalls fehlt auch dieser Rest.
7. **Legal erlaubte Metadaten müssen wirklich ausreichend sein.** S194 braucht einen abhängigkeitsgeprüften engen Reducer. Ein Manifestvollständigkeitsflag darf fehlende Payload nicht verdecken. S186 lässt keinerlei heimliche Ausnahme zu.
8. **Shutdown ist keine universelle Erledigung.** S179 verlangt getrennte Prüfung von Exportlesbarkeit, lokalem Dienstende, Teilnehmerrestpflichten, möglichen Spätkosten und vereinbartem Beobachtungshorizont. Eine einzelne Abschlussnachricht deckt nicht alle offenen Operationen ab.
9. **Root und Kryptobruch begrenzen unabhängige Beweise.** GVR033 fällt, wenn derselbe Root oder die allein gebrochene Kryptobasis auch die angebliche Gegenquelle kontrolliert. Es wird nur der tatsächlich verwahrte konkrete Fakt geschützt, nicht jede spätere Historie.
10. **Alle vorgeschlagenen Nachweise sind noch auszuführen.** Katalog und Zweige nennen unterscheidende Beobachtungen; hier liefen nur Artefaktprüfungen. Weitere isolierte Experimente brauchen einen neuen Auftrag. Externe Teilnehmer, reale Personen, Veröffentlichungen und Aktuatoren bleiben ausdrücklich gesondert genehmigungspflichtig.

## Übernommene Reviewauflagen und Abschlussprüfung

Übernommen wurden insbesondere BR01; QF03/QF04 für unabhängige Holds und enge Recovery-Entrypoints; QF06/QF07 für tatsächlich durchgesetzte Guards und begrenzte Abschlüsse; QF08 für Diagnose versus operative Lösung; QF09/QF10 für Vergleichsbedingungen und Arbeitsproduktion; QF12/QF14 für Informationsinventar und Ergebnisgrenzen; QF13 für historische Wirkung versus Schaden; QF15 für echte Rückkopplung; QF16 für Retirementgrenzen; QF17 für Source-Evidenz. Die Residenzkorrektur wurde wie oben beschrieben auch für S133 berücksichtigt.

Die mechanische Abschlussprüfung ergab:

- genau S101–S200 in Szenarioreihenfolge, jeweils einmal, keine fremden IDs;
- genaues CSV- und JSON-Schema, nichtleere Pflichtfelder, erlaubte Typ-/Statusvokabulare;
- eindeutige JSON-Schlüssel, eindeutige Residue-IDs und Zustandsnamen innerhalb jedes Falls;
- 214 Zweige, alle begründet mit Bedingungen, Dauermechanismus und widerlegender Beobachtung;
- alle 38 Kandidaten definiert und verwendet, alle Referenzen vorhanden;
- `bedingt`/`teilweise` immer mit Referenz; `keines`/`ungeklaert` hier immer ohne Referenz und mit begründeter Architekturgrenze;
- historische Herkunft exakt gegen `coverage.csv` **und jede Ursprungstrajektorie** geprüft;
- alle fünf alten offenen Fälle bedingt verzweigt, BR01-Einzelfehler und S190 ohne Residue ausdrücklich geprüft;
- UTF-8, semikolongetrennter CSV-Roundtrip ohne Feldverlust;
- alle fünf eingefrorenen Originale SHA-256-identisch;
- eigenes Ausgabeverzeichnis enthält nur die drei beauftragten Dateien.

Materialentscheidungen zum bedingten Ansatz, zu offenen Fällen und zu den Vertrags-Splits sind im zugewiesenen Run dokumentiert. Die Selbstprüfung korrigierte außerdem zu starke Wiederverwendungen: allgemeiner Resilienzanspruch ist keine Effekthülle; aktive Klonhistorie ist kein wirkungsfreier Offlineentwurf; lesbare Prüfakte benötigt für bloße Nachprüfung keine politische Durchsetzungskraft.

**Bestandene Strukturprüfung bedeutet konsistente Abgabe, nicht beobachtete Resilienz, vollständige Dynamik oder bewiesene Attraktoren.** Diese Einschränkung ist selbst Teil der Antwort auf S200.
