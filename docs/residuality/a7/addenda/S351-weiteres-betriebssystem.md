# S351 — Factory soll ein weiteres Betriebssystem unterstützen

**Nachtrag auf ausdrücklichen Nutzerhinweis, 2026-09-09.** Koordinatoranalyse, noch kein unabhängiger Review und kein ausgeführter Plattformtest. Der eingefrorene 350er-Korpus bleibt unverändert. Zusammen mit diesem Nachtrag sind **351 Stressoren erfasst**, aber nicht alle nach demselben Verfahren geprüft. Die bisherigen Zahlen 855 Zweige und 153 Kandidaten beziehen sich weiterhin auf den 350er-Korpus.

## Der Stressor

Factory läuft in einer bestimmten Umgebung. Nun soll dieselbe Aufgabe auf einem weiteren Betriebssystem funktionieren — beispielsweise von macOS auf Linux oder auf **natives Windows**. Alternativ muss nach Verlust des ursprünglichen Rechners auf das noch verfügbare Betriebssystem ausgewichen werden.

**Die gebrochene Annahme:** Gleicher Rust-Code, gleiche Datei oder gleicher Befehlsname bedeutet nicht automatisch gleiche Identität, gleiche Zugriffsrechte, gleiche Prozesskontrolle oder gleiche Wiederherstellungsfähigkeit.

Zwei Auslöser sind zu unterscheiden:

- **Geplante Erweiterung:** Der alte Rechner und sein rechtmäßig nutzbarer Betrieb bleiben verfügbar. Der neue Port darf scheitern, ohne den vorhandenen Dienst zu zerstören.
- **Erzwungene Wiederherstellung:** Der alte Rechner, seine Schlüssel oder seine Laufzeit fehlen. Ein Rückgriff auf diese Dinge ist dann kein verfügbares Residue.

Ein neues OS ist nicht von sich aus ein Attraktor. Ein Compilerfehler ist zunächst eine Barriere. Attraktion braucht eine begründete Rückkopplung, etwa sich gegenseitig erzeugende Plattform-Sonderlösungen.

## Konkrete heutige Anknüpfungspunkte

Zeitgebundene Quellinspektion am laufend veränderten Stand `e93597f` plus Arbeitskopie:

- [`factory-paths`](../../../../crates/factory-paths/src/lib.rs) importiert ungeschützt `std::os::unix::fs::MetadataExt` und bestimmt Laufzeit-Dateiidentität mit Gerät/Inode. Die Kommentare erklären ausdrücklich den kooperativen macOS-/APFS-Ausgangspunkt und dass dies keine feindliche Sicherheitsgrenze ist.
- [`daemon/socket.rs`](../../../../crates/factory-daemon/src/socket.rs), [`client.rs`](../../../../crates/factory-daemon/src/client.rs) und [`server.rs`](../../../../crates/factory-daemon/src/server.rs) verwenden ungeschützt `std::os::unix::net`. Ein nativer Windows-Port ist deshalb nicht allein durch Austauschen einer Pfadzeichenfolge erledigt. Es wurde kein Windows-Kompilierungsversuch ausgeführt.
- [`restore.rs`](../../../../crates/factory-recovery/src/restore.rs) erhält alte Leases und Versuchsevidenz nur innerhalb des vorhandenen Bestands. Die [lokale P02-Teilprüfung](../validation/p02/README.md) zeigt konkret: Ein vor dem Versuch erstellter Snapshot kennt dessen spätere Existenz nicht. Ein Umzug auf ein anderes OS macht diese Wissenslücke nicht kleiner.

Linux kann Unix-Schnittstellen bereitstellen. Daraus folgt hier noch kein bestandener Linux-Betrieb: Runtime, Dateisystem, Rechte, Paketierung und Prozesslebenszyklus müssen trotzdem geprüft werden. **WSL ist ein eigenes Profil**, kein Nachweis für natives Windows; insbesondere gemeinsam genutzte Windows-Laufwerke können andere Bedingungen haben als das Linux-Dateisystem im Gast.

## Mögliche Zustände und zugeordnete Residues

Die IDs bleiben die bestehenden Kandidaten, keine neuen Synonyme. Wiederverwendung gilt nur unter den vollständigen [Kandidatenbedingungen](../generated/residues.md). Die sieben Zweige sind nicht erschöpfend und können teilweise gleichzeitig gelten.

<a id="s351-b01"></a>
### S351.B01 — Der neue Port baut oder startet nicht

**Art:** extern bedingte Plattformbarriere, kein belegter Attraktor. Das geforderte Zielsystem bietet die benötigte Schnittstelle nicht; ein bewusster Verzicht auf die neue Freigabe wäre ein zusätzlicher Policy-Halt.

OS-spezifische Schnittstelle fehlt → Binary lässt sich nicht bauen oder notwendige Runtime startet nicht → die angeforderte neue Nutzung bleibt unmöglich.

**Residue: GVR016**, aber nur wenn ein vollständiger, weiterhin rechtmäßig nutzbarer Lauf-/Bausatz **und eine kompatible verfügbare Umgebung** tatsächlich erhalten sind. Dann kann die alte Variante weiterbetrieben oder reproduziert werden. Bei Verlust der einzigen alten Plattform entfällt genau dieser Rückweg. Ein Git-Repository allein reicht nicht.

**Architekturfolge:** M10 verwaltet die konkrete Plattform-/Versionskombination, M09 deren Wiederherstellungsweg. Kein automatisches Upgrade der noch funktionierenden Installation, um den neuen Port zu erzwingen.

**Gegenprobe:** Alte Plattform abschalten und Abhängigkeiten offline nehmen. Gibt es dann keinen tatsächlich ausführbaren Satz mehr, darf der Test keinen fortgesetzten Dienst behaupten.

<a id="s351-b02"></a>
### S351.B02 — Bestände bleiben lesbar, Ausführung bleibt gesperrt

**Art:** begrenzter Halt, keine Anziehungshypothese.

Ein geprüfter Leser läuft auf dem Zielsystem, Runtime oder Kontrollvertrag aber nicht → der Benutzer kann vorhandene Arbeit untersuchen → neue Worker und Wirkungen bleiben aus.

**Residue: GVR020**, wenn passende Daten, Decoder, legitimer Zugriff und tatsächlich effektfreier Lesepfad vorhanden sind. Ein Versionsfehler ohne funktionierenden Leser ist **nicht** dieses Residue. Die heutige Datenbank wird dadurch nicht rückwirkend zum kanonischen Eventlog erklärt.

**Architekturfolge:** M01/M07 erhalten lesbare Fakten und Inhalte; M10 unterscheidet Lese- von Schreibfähigkeit; M05 startet keine ungeprüfte Runtime. Der lokale Wiederherstellungszugang darf nicht von genau dem fehlenden optionalen Harness abhängen.

**Gegenprobe:** Runtimeadapter entfernen, aber Leser behalten. Benannte Inhalte müssen weiter prüfbar bleiben, ohne Writer, Plugin oder automatische Migration anzustoßen.

<a id="s351-b03"></a>
### S351.B03 — Die benannte Plattformkombination trägt den Vertrag

**Art:** erfolgreicher Abschluss einer begrenzten Portierung, kein neu entdeckter Attraktor.

Port gebaut → dieselben fachlichen Grenzfälle auf dem Zielsystem geprüft → die **ausdrücklich geprüfte** Kombination ist nutzbar.

**Residue: GVR016**, hier für den tatsächlich vorhandenen Zielplattform-Satz. Dazu gehören Dependencies, Rechte und passende Werkzeuge. Die erfolgreiche Portierung beweist nicht, dass jede künftige OS-, Dateisystem- oder Runtimeversion ebenfalls passt.

**Architekturfolge:** M10 veröffentlicht keinen pauschalen Haken „Windows“, sondern einen versionierten Nachweisumfang. M08 hält Resultate und Testlücken auseinander. Dieser Zustand ist ein Vorschlag, kein heute ausgeführtes Linux-/Windows-Ergebnis.

**Gegenprobe:** Ein absichtlich defekter Rechte- oder Aliasadapter muss denselben Vertragstest auf Rot setzen. Nur ein erfolgreicher Build ist ein Scheinerfolg.

<a id="s351-b04"></a>
### S351.B04 — Andere Pfadidentität erlaubt konkurrierende Schreiber

**Art:** Schreibkonflikt; erst zusätzliche Reparatur-Rückkopplung könnte einen Kreislauf begründen.

Zwei Namen oder Volumeansichten werden fälschlich als zwei Arbeitsverzeichnisse behandelt → zwei Worker erhalten Zugriff auf denselben Gegenstand → widersprüchliche Änderungen entstehen. Alternativ werden zwei verschiedene Verzeichnisse fälschlich zusammengelegt und legitime Arbeit unnötig gehalten.

**Residue: OPR003**, sofern ein unabhängig intakter, lesbarer Sicherungspräfix mit bekanntem Stand vorhanden ist. Damit kann ein Bearbeiter frühere Fakten untersuchen. Das verhindert den Konflikt nicht nachträglich und klärt keine späteren Außenwirkungen. Ohne diesen Bestand bleibt auch diese Restfähigkeit unbelegt.

**Architekturfolge:** M07 kapselt native Dateiidentität; M05 verwendet sie für lokale Besitzprüfung. Gespeicherte Unix-Gerät/Inode-Werte sind keine portable Identität für ein neues OS oder einen anderen Rechner. M06/M16 müssen einen wirklichen externen Schreiberausschluss separat begründen.

**Gegenprobe:** Groß-/Kleinschreibung, Symlinks beziehungsweise Junctions, unterschiedliche Volumeansichten und Löschen/Neuanlegen prüfen. Ein zweiter Name darf keine zweite aktive Lease für denselben Gegenstand ermöglichen. Kein Netzlaufwerk wird still wie ein lokales Volume behandelt.

<a id="s351-b05"></a>
### S351.B05 — Plattform-Sonderlösungen erzeugen immer neue Sonderlösungen

**Art:** bedingte Attraktorhypothese, nicht empirisch nachgewiesen.

Schneller OS-Sonderfall umgeht gemeinsamen Vertrag → die andere Plattform bricht → nächste lokale Ausnahme → Fehlerbilder und Verhalten driften → weitere Ausnahme erscheint erneut als schnellster Ausweg. Der Kreislauf ist nur begründet, wenn diese Änderungen wirklich weitere Reparaturarbeit erzeugen und das gemeinsame Prüfen verdrängen. Ein einzelner notwendiger OS-Adapter ist kein solcher Kreislauf.

**Residue: GVR020**, falls wenigstens der unveränderte Bestand mit einem geprüften Leser interpretierbar bleibt. Das bewahrt Untersuchung, nicht automatisch einen wartbaren Schreibpfad. Ohne passenden Leser ist auch dieser Rückzug nicht vorhanden.

**Architekturfolge:** Gemeinsame Domänenregeln und Vertragstests bleiben zentral. Native Mechanik ist gekapselt, nicht die fachliche Bedeutung. Versionsgebundene Fähigkeiten statt verstreuter Ausnahmen im Task-Lebenslauf.

**Gegenprobe:** Nach Entfernen des ursprünglichen Portierungsfehlers Nachfrage, Teamkapazität und Regeln konstant halten. Wenn die Sonderarbeit ausläuft, war es ein endlicher Umbau, kein belegter anziehender Zustand. Wartungsdaten über mehrere Releases wären erforderlich.

<a id="s351-b06"></a>
### S351.B06 — Bytes sind umgezogen, der einzige Schlüssel ist unwiederbringlich verloren

**Art:** Verlust, kein Attraktor.

Datenbank und verschlüsselte Inhalte wurden kopiert → nur das alte OS-Konto konnte entschlüsseln → dieses Konto und sämtliche benötigten Schlüsselkopien sind endgültig verloren.

**Residue: keines für die Entschlüsselung dieser Inhalte.** Unverschlüsselte Metadaten können unter anderen Bedingungen lesbar bleiben; sie ersetzen weder den Schlüssel noch die fehlende Nutzfunktion.

**Architekturfolge:** M09/M14 müssen **vorher** einen legitimen, tatsächlich getesteten Übergabeweg vereinbaren. Kein Export von Credentials in Tasktexte, Logdateien oder Repository. Betriebssystemübergreifende Portabilität darf nicht durch Abschalten der Geheimnisgrenze erkauft werden.

**Gegenprobe:** Nur verschlüsselte Kopie ohne jeden gültigen Schlüssel bereitstellen. Der Versuch muss die Entschlüsselungsfähigkeit verneinen, statt einen zusätzlichen Verwahrer zu erfinden.

<a id="s351-b07"></a>
### S351.B07 — Das Zielsystem kann den geforderten Schutz nicht tragen

**Art:** begründeter Halt beziehungsweise Einsatzgrenze.

Binary läuft → tatsächliche Kanalrechte, Prozesskontrolle oder Zugriffstrennung reichen für das zugesagte Profil nicht → gefährdete neue Handlungen werden nicht freigegeben.

**Residue: OPR001**, wenn Taskdaten, Decoder und berechtigter lokaler Zugriff erhalten sind. Dann bleibt ein konkreter Auftrag lesbar und für eine legitimierte Entscheidung verfügbar. Der lesbare Auftrag beweist weder sicheren Schreibbetrieb noch Geheimhaltung gegenüber einem gleich mächtigen Angreifer.

**Architekturfolge:** M02/M14 beschränken die betroffene Fähigkeit; M11 benennt den fehlenden Vertrag. Kein stiller Fallback auf öffentlichen TCP-Zugang, umfassende Administratorrechte oder ungeschützte Schlüsseldateien. Bereits eingetretene Offenlegung wäre ein eigener Verlustzweig, nicht durch diesen Halt rückgängig gemacht.

**Gegenprobe:** Einen Prozess mit dem konkret ausgeschlossenen Benutzer-/Angreiferrecht ansprechen lassen. Reiner Admin-Test oder derselbe Testbenutzer auf beiden Seiten genügt nicht.

## Was die modulare Zielarchitektur daraus lernen muss

**Kein siebzehntes Universalmodul „OS“ und kein Dienst pro Betriebssystem.** Stattdessen werden bestehende Grenzen konkreter:

| Eigner | OS-abhängiger Vertrag | Gemeinsame Bedeutung bleibt |
|---|---|---|
| M07 | Native Dateiidentität, Namensauflösung, Dateisystem- und Schreibgrenzen | derselbe Gegenstand ist nicht zweimal frei; Factory schreibt Dateien nur unter `.factory/` |
| M05 | Prozess-/Nachkommenkontrolle, lokale Installationsexklusivität, Runtimebeobachtung | Start, unbekannte Liveness, Stopp und tatsächlich beendete Arbeit bleiben verschieden |
| M10 | Aktivierte API-Transporte, Runtime-/Harnesskombination, Paketierung | ein versioniertes logisches API und deklarierte Fähigkeiten, keine implizite globale Pluginpflicht |
| M02/M14 | Lokaler Clientzugriff, native Rechte, Secretprovider | aktuelle legitime Befugnis ist nicht bloßer Passwort- oder Prozessbesitz |
| M04 | Suspendverhalten, monotone Zeit, Uhränderung, optionaler OS-Scheduler | Frist, Zeitplan und Handlungsberechtigung werden nicht gleichgesetzt |
| M01/M09 | Datei-/SQLite-Dauerhaftigkeit, Austausch von Sicherungen, Decoder und Schlüsselzugang | lesbare Vergangenheit ist keine aktuelle Erlaubnis und keine Prozessbeobachtung |
| M08/M11 | Native Vertragstests und sichtbarer Nachweisumfang | „baut“, „startet“, „kann lesen“ und „hält den Betriebsvertrag“ sind getrennte Ergebnisse |

Der wiederverwendbare Kern behält Task-, Autoritäts-, Ereignis- und Effektsemantik. Native Hostmechanik für Bootstrap und Subprozessaufsicht bleibt klein und gekapselt. Runtime, Harness, API-Transport, Secretprovider und OS-Scheduler gehören hinter ihre jeweiligen versionierten Pluginverträge; jeder Plugin läuft als beaufsichtigter Unterprozess. Ein Bootstrap darf nicht den laufenden Plugin voraussetzen, den er erst starten soll.

Ein Windows-Kanal braucht einen eigenen Zugriffsschutz- und Ressourcenvertrag. Ein alternativer IPC-Namensraum ist keine automatische Ausnahme von der `.factory/`-Dateischreibregel. Ob und wie ein konkreter Transport zulässig umgesetzt wird, ist **vor** seiner Einführung zu entscheiden. Einmaliger lokaler Mutationsbesitz ist außerdem keine maschinenübergreifende Schreibautorität.

## Prüfung P13 — Betriebssystemwechsel und Erhalt derselben Verträge

**Neu vorgeschlagen, nicht ausgeführt.** Matrix mindestens nach OS/Version, CPU-Architektur, Dateisystem/Mount, Benutzer-/Rechtemodell, Transport, Runtime/Harness und Secretprovider. Nicht jeder denkbare Matrixpunkt muss unterstützt werden; nicht geprüfte Kombinationen heißen nicht unterstützt oder ungeprüft.

1. Offline bauen und starten; fehlenden optionalen Adapter gezielt entfernen. Prüfen, ob der deklarierte Lesemodus noch erreichbar ist.
2. Native Alias-/Leasefälle und zwei konkurrierende lokale Daemons prüfen. Derselbe Vertrag muss auf beiden Plattformen gelten; nicht jedes Dateisystem bietet dieselben Voraussetzungen.
3. Zugriff durch einen anderen Benutzer, Nachkommen nach Stopp, Suspend und späte Antworten prüfen. Keine administrative Allmacht im Test als Benutzerschutz ausgeben.
4. **P02 auf jeder unterstützten Kombination wiederholen** und zusätzlich die fehlende aktuelle AuthoritySource sowie wirkliche Teilnehmerannahme prüfen. Die vier bisherigen macOS-Teiltests allein reichen dafür nicht.
5. Eine Sicherung von OS A auf OS B nur lesend öffnen. Keine Runtimeidentität, Lease oder Freigabe allein aus der transportierten Datei reaktivieren. Schlüsselübergabe nur innerhalb eines separat genehmigten Verwahrvertrags testen.
6. Mindestens eine absichtlich defekte Implementierung je kritischem Vertrag muss erkannt werden. Ein Cross-Compile oder ein Container-Build ersetzt die nativen Semantiktests nicht.

**Begrenzter Abschluss:** Ein benanntes Plattformprofil besteht seinen erklärten Vertrag oder wird gezielt eingeschränkt. Offen bleiben konkrete Ziel-OS, Mindestversionen, garantierte Fähigkeiten und deren Wartungsbudget. Dieser Nachtrag entscheidet nicht, dass Linux, Windows oder jede Runtime jetzt unterstützt werden muss.
