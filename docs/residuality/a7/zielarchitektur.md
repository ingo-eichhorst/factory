# Zielarchitektur A7 — viele Restfähigkeiten, ein übersichtliches System

**Status: überarbeiteter Entwurf aus der Analyse aller 350 Stressoren. Keine Implementierungs- oder Betriebszusage.** A7 ist der aktuelle Architekturvorschlag dieses Analysepakets und löst dessen pauschale A3/A4-Verdichtung als Einstieg ab. Bestehende angenommene ADRs werden nicht stillschweigend geändert. Wo A7 mehr verlangt, ist das unten als zu entscheidender Vertrag benannt.

## 1. Was sich gegenüber der bisherigen Zielarchitektur ändert

Bisher wurden viele Stressoren unter 17 sehr breiten Residue-Kategorien erklärt. Danach wurden 19 ausdrücklich benannte Attraktorhypothesen vertieft. Das ließ andere dauerhafte Zustände und Restfähigkeiten zu wenig sichtbar werden.

A7 betrachtet **jeden Stressor mit seinen bedingten Zweigen**. Ein verlorener Schlüssel ist beispielsweise nicht dasselbe wie verlorene Bytes. Ein erhaltenes Versuchsprotokoll ist nicht dasselbe wie ein Teilnehmer, der doppelte Zahlungen verhindert. Ein Alarm ist nicht dasselbe wie ein erreichbarer Mensch, und eine Quittung ist keine Reparatur.

Die Konsequenz lautet nicht „für alles einen neuen Dienst bauen“, sondern:

> **Gemeinsame technische Bausteine verwenden, ihre unterschiedlichen Schutzverträge aber ausdrücklich erhalten.**

Die [vollständige Zuordnung](generated/stressoren.md) erklärt alle 350 Fälle. Der [Residue-Katalog](generated/residues.md) enthält Gegenstand, nutzbare Fähigkeit, Voraussetzungen, Ausfallgrenze und Prüfvorschlag. Die [Modulzuordnung](generated/modules.md) zeigt, wer den jeweiligen Vertrag verantwortet. Aktuelle Zählwerte stehen im [Zuordnungsstand](generated/summary.md); sie sind keine Resilienzkennzahl.

## 2. So sieht das System aus

```text
Menschen, CLI, Dashboard, Agenten
            │
            ▼
  Beaufsichtigte API-Transport-Plugins
  Lokaler IPC-Zugang bleibt als Wiederanlaufweg erhalten
            │ authentisierter, begrenzter Aufruf
            ▼
┌──────────────── EIN Factory-Daemon ─────────────────────┐
│ M02 Identität und Befugnisse                            │
│ M03 Arbeit, Kapazität und Kosten    M04 Zeitabsicht      │
│ M05 Ausführung, Besitz und Kontrolle                    │
│ M06 Wirkungskoordination                               │
│                                                        │
│ M07 Inhalte und Kontext      M08 Abnahme und Nachweise  │
│ M09 Wiederherstellung        M10 Versionsverträge       │
│ M11 Diagnose und Messung                               │
│                                                        │
│ M01 Faktenkern: atomare Ereignisse und reine Wiedergabe │
└──────────────────────┬─────────────────────────────────┘
                       │ begrenzte, versionierte Aufrufe
              ┌────────┴────────┐
              ▼                 ▼
      Runtime-/Harness-     M16 Fach- und Wirkungsadapter
      Plugins               + tatsächliche Teilnehmer
                            + ggf. spezialisierte Controller

Querschnitt mit ausdrücklich getrennten Grenzen:
M14 Geheimniszugang und reale Einsatzisolation
M15 rechtmäßig entschiedene Aufbewahrung und Datenverfügung
M13 menschliche Fallübergabe über bestehende Tasks und Clients

Außerhalb der betrachteten Host-/Angreifergrenze, falls eingerichtet:
M12 unabhängiger Beobachter → erlaubter Alarmweg → legitimer Empfänger
```

**16 Module bedeuten 16 Verantwortungsbereiche, nicht 16 zusätzliche Server.** Viele sind interne Rust-Bibliotheken beziehungsweise klar begrenzte Teile desselben Daemons. Die Zuordnung schreibt keine Zahl von Crates vor. API-Transporte, Runtimes, Harnesses und externe Integrationen bleiben beaufsichtigte Unterprozesse. Menschen, fremde Dienste, Betriebssystemgrenzen und ein unabhängiger Beobachter werden nicht durch das Diagramm zu Kernel-Code.

Das Subprozessprinzip isoliert bestimmte Abstürze. Es macht Prozesse unter demselben mächtigen Betriebssystembenutzer nicht zu einer sicheren Sandbox. Interne Modulgrenzen wiederum sind keine eigenen Strom-, Speicher- oder Angreifergrenzen.

## 3. Die Modulverträge in einfacher Sprache

| Modul | Zuständig für | Wofür es ausdrücklich nicht steht |
|---|---|---|
| **M01 Faktenkern und reine Wiedergabe** | Bestätigte Fakten, atomare Änderungen, Kommandoergebnisse, nachvollziehbare historische Rekonstruktion | Gespeicherte Geschichte beweist nicht die heutige Außenwelt. Wiedergabe startet keine Aktionen. |
| **M02 Identität und Befugnisse** | Richtiger Aufrufer, Projektbereich, Freigabegegenstand, gültige Befugnis und erkennbare Konflikte | Ein bekannter Name, Passwort oder alter Freigabebeleg ist keine neue Erlaubnis. |
| **M03 Arbeit, Kapazität und Kosten** | Endliche Arbeitsbestände, Herkunft von Unteraufträgen, gemeinsame Aufnahmegrenzen, belegte Kostenverpflichtungen | Eine Warteschlange schafft keine Kapazität. Ein unbekannter Preis ist keine sichere Kostenobergrenze. |
| **M04 Zeit und Terminabsicht** | Bedeutung einer Frist, zivile Wiederholung, Uhrunsicherheit, eindeutig disponierte Nachholung | Eine neue Uhr oder ein neuer Versuch verlängert keine fachliche Vollmacht. |
| **M05 Ausführung, Besitz und Kontrollzugang** | Prozesse, Aktivierungen, beobachtete Identität, Workspacebelegung, begrenzte Kommunikation und erreichbare Kontrolle | Parentende oder verlorene Verbindung beweisen nicht, dass alle Schreiber verschwunden sind. |
| **M06 Wirkungskoordination** | Absicht, tatsächlicher Versuch, unbekannter Ausgang, Abgleich und operationenspezifische Kompensation | Ein lokaler Eintrag liefert keine automatische Genau-einmal-Wirkung und keinen universellen Rollback. |
| **M07 Inhalte, Kontext und sichere Dateizugriffe** | Originalbytes, echte Objektidentität, Herkunft, tatsächlich gebundener Kontext und sichere erlaubte Schreibziele | Ein Pfad ist kein Inhalt; ein Hash ist keine fachliche Abnahme. |
| **M08 Fachliche Abnahme und Nachweise** | Passende Prüfkriterien, Gegenbelege, Teilbefunde und begrenzte Aussagen | Kein allwissender Prüfer und keine Betriebsfreigabe aus einer vollständigen Tabelle. |
| **M09 Wiederherstellung und Weitergabe** | Nutzbarer Wiederanlaufbestand mit Inhalt, Leser, Werkzeugen, Zugriff, Rechten und überprüfter Übergabe | Ein Manifest ersetzt keine fehlenden Schlüssel. Ein Export beendet keine fremden Aufträge. |
| **M10 Versions- und Integrationsverträge** | Tatsächlich gestarteter Code, Feldbedeutung, Aktivierung, Kompatibilität und bekannte Abhängigkeiten | Signiert heißt nicht harmlos. Optional für einen Bereich heißt nicht Startpflicht für alle. |
| **M11 Diagnose und Messung** | Begrenzte Rohmessungen, konkrete Gesundheitsbefunde, sichere Anzeige und sichtbare Messlücken | Ein grüner Hostping beweist keinen Commit und keine richtige Geschäftsentscheidung. |
| **M12 Unabhängige Beobachtung** | Begrenzte aktuelle Evidenz außerhalb der jeweils benannten Ausfalldomäne | Kein automatisches Reparatur-, Schreib-, Ersatzstart- oder Geschäftsrecht. |
| **M13 Vorfälle und menschliche Übergabe** | Offene Fallakte, Meldung, Empfang, zuständige Bearbeitung und geprüfter Abschluss | Eine Quittung ist keine Reparatur. Ein Rollenname erzeugt keine legitime Nachfolge. |
| **M14 Geheimniszugang und Einsatzisolation** | Zweckgebundene Schlüsselnutzung und tatsächlich durchgesetzte OS-/Verwahrgrenzen | Scope-Namen und Unterprozesse allein schützen nicht vor gemeinsamem privilegiertem Zugriff. |
| **M15 Aufbewahrung und Datenverfügung** | Konkret zulässige Nutzung, Aufbewahrung, Export und Löschung sowie deren begrenzter Nachweis | Kein Rechtsautomat, keine universelle Fernlöschung und keine heimliche Backupbereinigung. |
| **M16 Fach- und Wirkungsadapter** | Fachliche Bedeutung, Annahmebelege und wirksame Schutzbedingungen beim echten Teilnehmer | Eine Testquittung ersetzt keinen Teilnehmervertrag. Factory ist kein Echtzeit-Sicherheitscontroller. |

Die [maschinenlesbaren Verträge](modules.csv) nennen zusätzlich Datenhoheit, Schnittstelle, eingeschränkten Betrieb und Prüffrage. Die [einzelnen Zuordnungen](module-bindings.csv) werden ausdrücklich durch den Koordinator festgelegt, nicht aus Wortähnlichkeit errechnet.

### Warum diese Aufteilung modular ist

Jeder Residue-Kandidat hat **einen primären Vertragseigner**. Andere Module liefern klar benannte Mitwirkung. Das bedeutet nicht, dass der primäre Eigner alles selbst speichert oder gegen alle Störungen schützt.

Beispiel: M06 führt das lokale Versuchsprotokoll. M16 liefert einen echten Teilnehmerbeleg. M02 prüft die Befugnis für eine neue Handlung. Alle drei können gemeinsam gebraucht werden, ohne dieselbe Fähigkeit zu sein. Gemeinsame Speicherung von Kernfakten bleibt bei M01.

Ein Adapter darf die Fachbedeutung seiner Operation kennen; er darf keine zweite Factory-Auftragsverwaltung oder direkte Schreibverbindung zum Kernstore bekommen. Ein Client darf verständlich darstellen; er entscheidet nicht heimlich anders über Freigaben als der Daemon.

## 4. Kleine gemeinsame Begriffe statt paralleler Systeme

Die Module teilen wenige klar abgegrenzte Begriffe. Die folgenden Namen beschreiben Entwurfsinhalte, keine bereits ratifizierten API-Typen:

- **Arbeitsidentität:** ursprünglicher Auftrag, Ausführung, Delegationsherkunft und einzelner Versuch sind unterscheidbar. Ein neuer Versuch darf kein versteckter neuer Hauptauftrag sein.
- **Gebundener Handlungsgegenstand:** konkrete Eingabe, aufgelöstes Ziel, zuständiger Bereich, Zweck, Umfang und Freigabestand. Veränderliche Verweise dürfen die bestätigte Bedeutung nicht austauschen.
- **Zeitvertrag:** technische Wartezeit, fachliche Gültigkeit, Freigabeablauf und zivile Terminabsicht bleiben getrennt.
- **Beobachtungsbeleg:** Gegenstand, Ursprung, Zeitpunkt beziehungsweise Zeitunsicherheit, Geltungsbereich und bekannte Lücken. Ein Beleg kann einer anderen Aussage widersprechen.
- **Wirkungsstand:** geplant, versucht, unbekannt, tatsächlich angenommen oder abgelehnt und gegebenenfalls kompensiert sind verschiedene Aussagen.
- **Inhaltsreferenz:** bezeichnet benötigte unveränderte Bytes und deren Zugang, nicht automatisch Wahrheit oder Nutzbarkeit.
- **Prüfvertrag:** welche Eigenschaft für welchen Zweck untersucht wurde, was fehlte und welcher Gegenbeleg die Annahme ändern darf.

Diese Begriffe sollen bestehende Task-, Session-, Scope-, Ereignis- und Operationsverträge konkretisieren. **Kein neuer allgemeiner Workflowmotor und kein v1-Message-Store.** Störungsfälle verwenden vorhandene Tasks mit Belegen und Zuständigkeiten; Benachrichtigungsversuche sind zugeordnete Vorgänge, keine neue allgemeine Gesprächsplattform.

## 5. So arbeiten die Module zusammen

### Ein neuer Auftrag mit Außenwirkung

1. Ein Transport begrenzt und authentisiert den Aufruf. Vom Client behauptete Actor-Namen werden nicht einfach geglaubt.
2. M02 prüft Identität, Projektbezug und konkrete Befugnis. Falls persönliche Zustimmung belegt werden soll, muss der Vertrauensvertrag diese tatsächlich tragen; ein kompromittierter Transport kann nicht sein eigener unabhängiger Zeuge sein.
3. M03 prüft Arbeits-, Versuchs- und Kostenaufnahme. M04 liefert gültige Zeitbedingungen. M07 bindet die tatsächlich betroffenen Inhalte.
4. M06 plant den konkreten Schritt. M01 speichert die erforderlichen Fakten, Reservierungen und den Versandauftrag atomar, bevor ein neuer externer Versuch darauf gestützt wird.
5. M05 vermittelt den begrenzten Aufruf an M16. Dort gelten die tatsächlich zugesagten Annahme-, Identitäts-, Ablauf- und Ausschlussbedingungen.
6. Ergebnisbelege werden als neue Fakten festgehalten. Antwortverlust ist nicht automatisch Ablehnung. M08 prüft bei Bedarf den fachlichen Erfolg, statt jeden erfolgreichen Transport zum erfolgreichen Auftrag zu erklären.

**Keine Zaubertransaktion über das Netzwerk:** Eine lokale Datenbanktransaktion schließt keine entfernten Annahmerennen. Wenn ein Teilnehmer Widerruf, Ablauf oder Ausschluss nicht wirksam prüfen kann, darf das entsprechende Ende-zu-Ende-Versprechen nicht abgegeben werden. Der verbleibende Vertrag kann stattdessen Nichtwiederholung, manuelle Klärung oder bewusste Nichtunterstützung sein.

### Ein alter Snapshot wird wiederhergestellt

1. M09 prüft den tatsächlich verfügbaren Inhalt, Stand, Decoder, Zugang, Zielnamensraum und Platzbedarf. Eine geöffnete Datei wird nicht heimlich als Restore behandelt.
2. M01 kann nur die darin enthaltene Geschichte rekonstruieren. Ein nicht enthaltener späterer Versuch bleibt unbekannt.
3. M05 behandelt übernommene Prozess- und Workspaceangaben nicht als aktuelle Lebensbeweise. Reservierung, Prozessbeobachtung und tatsächlicher Schreibentzug bleiben verschiedene Dinge.
4. M02 klärt aktuelle Befugnisse und mögliche spätere Widerrufe. M06 gleicht offene Wirkungen mit verfügbaren M16-Belegen ab.
5. Erst danach wird die konkret gerechtfertigte neue Arbeit zugelassen. Ein freigegebener neuer Versuch klärt nicht von selbst den Ausgang des alten.

Damit ist ein **lesbarer Wiederherstellungsbestand** nutzbar, auch wenn automatische Fortsetzung nicht gerechtfertigt ist. Das ist ein echtes eingeschränktes Ziel, nicht die Behauptung, der Ausfall sei folgenlos.

### Factory ist ganz ausgefallen

1. Ein ausdrücklich eingerichteter M12-Zeuge außerhalb der betroffenen Energie-, Netz- oder Verwaltungsgrenze bemerkt fehlende frische Evidenz.
2. Er führt seinen eigenen begrenzten Beobachtungsstand weiter. Er schreibt nicht direkt in eine zweite Kopie der Factory-Kern-Datenbank.
3. Ein genehmigter Alarmweg versucht einen legitimierten Menschen zu erreichen. M13 trennt späteren Empfang, übernommene Bearbeitung und geprüfte Reparatur.
4. Nach Rückkehr können externe Belege über die normale API mit Herkunft übernommen werden. Ausbleiben eines Signals erlaubt keinen automatischen zweiten Schreiber.

Ohne externen Zeugen gibt es kein externes Ausfallversprechen. Verschwinden alle passenden Beobachter, Wege und Empfänger, bleibt für diese Alarmfähigkeit kein brauchbares Residue. Anderes erhaltenes Material wird dadurch nicht automatisch ebenfalls vernichtet.

### Überlast trifft auf einen Stoppbefehl

M03 begrenzt Arbeit **über alle Produzenten hinweg**. M05 hält eine kleine Kontrollreserve und begrenzt Frames vor großer Allokation. M06 kann neue vermittelte Effekte zurückhalten. M11 darf Diagnose unter Last reduzieren und Lücken zeigen, aber nicht den Commit auf einen externen Tracesammler warten lassen.

Eine unmittelbar wirkende Sperre, eine dauerhaft gespeicherte Sperre und ein tatsächlich beendeter Teilnehmer sind drei unterschiedliche Ergebnisse. Bei voller SSD darf das System keinen erfolgreichen dauerhaften Widerruf bestätigen, nur weil es im Arbeitsspeicher neue Aufrufe stoppt. Die Neustartbarriere braucht einen gesonderten tragfähigen Vertrag.

## 6. Abhängigkeiten dürfen keine neuen Ausfallschleifen bilden

Die [Abhängigkeitstabelle](module-dependencies.csv) unterscheidet:

- **bootstrap:** Was für den benannten minimalen Startvertrag bereit sein muss.
- **command:** Was eine konkrete angeforderte Funktion benötigt.
- **optional:** Was nur die aktivierte Zusatzfähigkeit benötigt.

Diese Kanten sind **Schnittstellen- und Phasenverträge**, keine Forderung, jedes beteiligte Modul vollständig hochzufahren, bevor das erste helfen darf. Beispielsweise ist sichere primitive Dateiöffnung vor Storezugriff nötig; eine vollständige Artefaktverwaltung darf dadurch nicht zur Voraussetzung ihrer eigenen Datenbank werden.

### Minimaler Start

1. Erlaubte `.factory/`-Wurzel, exklusive Installation und kleine feste Bootstrapgrenzen prüfen.
2. Den verfügbaren Kernstand prüfen; lesbaren Stand, beschädigte Daten und fehlende aktuelle Außenweltevidenz nicht vermischen.
3. Den gebündelten lokalen Transport mit legitimer lokaler Authentisierung bereitstellen. Externe Identitätsanbieter und optionale Plugins dürfen diesen Weg nicht erst ermöglichen müssen.
4. Konkrete Diagnose zeigen. Optionale Aktivierungen werden erst danach und unter eigenen Aufnahmegrenzen behandelt.
5. Bestehende Arbeit und Autorität abgleichen, bevor neue Ausführung beginnt.

Ist selbst der Kernelbestand unlesbar, gibt es keine erfundene normale Taskabfrage. Ein begrenzter Bootstrap-Fehlerbericht oder eine ausdrücklich geregelte Nurlese-Diagnose ist dann das kleinere Ziel. Automatische Datenreparatur und heimliches Öffnen einer zweiten Schreibinstanz sind kein Ersatz.

**Die Startkanten werden auf Zyklen geprüft.** Das beweist nur die Konsistenz dieses Entwurfs. Ob reale Parser, globale Sperren, Aufrufketten oder Pluginstarts zusätzliche versteckte Abhängigkeiten bilden, muss ein Implementierungsversuch zeigen.

### Keine unbegrenzten Puffer als Verbindung zwischen Modulen

Für jeden Übergang werden maximale Nachrichten- und Inhaltsgröße, gleichzeitige Aufrufe, Wartefrist und Überlaufverhalten festgelegt. Grenzen gelten auch für die Summe aller Aktivierungen und Empfänger. Eine kleine Grenze pro Empfänger schützt nicht vor einer unbegrenzten Empfängerzahl.

Es gibt keine universelle Zahlenvorgabe ohne Messbasis. Bei verlorenen Messdaten darf M11 eine Lücke ausweisen. Bei für Kernfakten notwendiger Evidenz muss das System nötigenfalls weitere Arbeit ablehnen, statt still Vollständigkeit vorzutäuschen.

## 7. Konkrete Wiederverwendung ohne falsches Zusammenlegen

| Gemeinsame Umsetzung | Was trotzdem getrennt bleibt |
|---|---|
| M01/M07 speichern und referenzieren erhaltene Fakten und Bytes | Snapshot-Präfix, Originalartefakt, entschlüsselbarer Inhalt und fachlich abgenommenes Ergebnis sind nicht dasselbe. |
| M03 bietet gemeinsame Aufnahme und Reservierung | Arbeitsmenge, technische Versuche, Geld, Kaltstart und garantierter Dienstanteil haben verschiedene Einheiten und Grenzen. |
| M06 koordiniert Handlungsgeschichte | Lokales Intent, Teilnehmer-Deduplizierung, Schreibgeneration, aktuelle Freigabe und wirksame Kompensation sind unterschiedliche Verträge. |
| M08 bietet eine gemeinsame Beleg- und Abnahmeschnittstelle | Ein unabhängiger Mengenbezug, ein korrekt begrenzter Test und ein fehlendes Pflichtveto sind nicht austauschbar. |
| M12/M13 teilen Beobachtung und Fallbezug | Frische, Versandannahme, menschlicher Empfang, berechtigte Zuständigkeit und tatsächliche Behebung bleiben getrennt. |
| M09 bündelt Wiederanlauf und Übergabe | Vorhandene Sicherung, erreichbarer Schlüssel, passender Leser, Bauwerkzeug, Nutzungsrecht und echte Nachfolge können jeweils allein fehlen. |

Die ursprünglichen Kandidaten-IDs bleiben deshalb nachvollziehbar. Eine gemeinsame Bibliothek kann mehrere dieser Anforderungen erfüllen. Eine künftige echte Zusammenlegung von Residues braucht den Nachweis, dass Gegenstand, notwendige Bedingungen und Schutzgrenze übereinstimmen — nicht bloß einen ähnlichen Namen.

### Drei Unabhängigkeitsgrenzen müssen auch in den Schnittstellen gelten

Der Gegenreview hat hier notwendige Präzisierungen ergeben. Die [Teilverträge](contract-facets.csv) und [Betriebsmodi](operation-boundaries.csv) sind verbindlicher Teil dieses Entwurfs und begrenzen die allgemeinen Mitwirkungslisten:

1. **Erkennen ist nicht Eingreifen — BDR037.** M12 besitzt nur den unabhängigen Vergleich. Den zusammengesetzten Wiederherstellungsvertrag verantwortet eine legitimierte externe Recoveryrolle unter M13. Ein getrennt befugter OS-/Verwahradministrator (M14) und gegebenenfalls Teilnehmeradministrator (M16) müssen Schreibrechte tatsächlich außerhalb der Updategewalt entziehen können. Diese Akteure brauchen weder den kompromittierten Kern als Erlaubnisgeber noch Schreibrechte am Monitor. Fehlt ihre Befugnis oder Erreichbarkeit, bleibt nur Beobachtung, wie im zusätzlichen Zweig S350.B05. Ein zweiter Factory-Kern wird dafür nicht eingeführt.
2. **Lokale Sicherheit läuft ohne Factory — GVR035, BDR026, BDR027.** Die normale Kette M02/M06/M14 gilt nur für vorgelagerte Factory-Anfragen. Der qualifizierte lokale Interlock oder Notstopp hat eigene geeignete Energie, Sensoren, Befugnisse und Aktuatorgewalt. Seine Sicherheitsreaktion benötigt kein laufendes M01, M02, M06 oder M14. Eine spätere Belegübernahme kann warten. Das ist ein externer Controllervertrag, kein neuer Sicherheitskernel in Factory.
3. **Aktueller Widerruf kommt nicht aus einem alten Snapshot — GVR011.** Für dieses stärkere Profil wird ausdrücklich eine `AuthoritySource` konfiguriert: eine legitimierte Widerrufsstelle verwaltet den maßgeblichen Principal-/Scope-/Grantstand außerhalb der rückgesetzten Factory-Stores. M16 vermittelt die Abfrage; M02 prüft Quelle, konkreten Gegenstand, Revision, Frische und aktuelle lokale Regeln. Vertrauensanker und enges Abfragerecht dürfen nicht von dem gerade strittigen alten Grant abhängen. Andernfalls liegt ein ungelöster Zugangszyklus vor. M01 bleibt historische Evidenz. Fehlt eine frische Quelle, darf nur die konkrete neue Handlung gehalten werden — kein aktueller Widerrufsbeleg wird erfunden. Für Gültigkeit bis zur entfernten Annahme braucht es außerdem tatsächliche Teilnehmerprüfung; ein frischer Versandcheck allein schließt das Rennen nicht.

Die AuthoritySource ist kein zweiter Taskstore und kein standardmäßig eingerichteter externer Dienst. Ihre Betreiberrolle, Daten, Vertrauensgrenze und Kosten brauchen einen gesondert vereinbarten Vertrag. Ohne ihn besitzt das Einsatzprofil diese stärkere Wiederherstellungsfähigkeit nicht.

## 8. Einsatzprofile statt stiller Universalversprechen

### Lokaler kooperativer Betrieb

Ein vertrauenswürdiger Betriebssystembenutzer, ein mutierender Daemon, begrenzte Plugins und lokaler Bedienweg. Das ist keine harte Abschottung gegen bösartigen Code unter demselben Benutzer. Ohne eingerichtete externe Beobachtung bleibt Rechnerausfall von Factory selbst nicht meldbar.

### Begrenzt unbeaufsichtigter Betrieb

Zusätzlich tatsächlich unabhängige Beobachtung, endliche Alarmregeln, legitime Vertretung, deklarierte Ressourcen-/Kostenbudgets und geprüfter Wiederanlauf. Menschenabwesenheit erweitert keine Freigaben. Nicht entscheidbare Arbeit bleibt gehalten; das kann das korrekte Betriebsergebnis sein.

### Lange Aufbewahrung und Portabilität

Zusätzlich konkrete Inhalts-, Decoder-, Schlüssel-, Werkzeug- und Rechtsinventare sowie geprüfte Übergabe. Zielzeiten und tolerierte Datenlücken gelten für benannte Inhalte, nicht pauschal für „alles“. Factory löscht keine Sicherungen automatisch, um diese Ziele schöner aussehen zu lassen.

### Ein weiteres Betriebssystem ist ein eigenes geprüftes Profil

Der additive [Stressor S351](addenda/S351-weiteres-betriebssystem.md) präzisiert eine bisher nur allgemeine Portabilitätsannahme. Gemeinsame Domänenregeln bleiben im Kern; native Mechanik für Dateiidentität (M07), Prozessbesitz und Bootstrap (M05), Zeit (M04) sowie Zugriff und Geheimnisse (M02/M14) wird gekapselt. API-Transporte, Runtimes, Harnesses und OS-Scheduler bleiben hinter ihren versionierten Pluginverträgen. Es entsteht weder ein Betriebssystem-Sonderkernel noch ein zusätzlicher Dienst pro OS.

M10 deklariert konkrete Kombinationen aus OS, Dateisystem, Rechtemodell, Transport und Runtime statt eines pauschalen Portabilitätsversprechens. M08 verlangt native Vertragstests; M11 unterscheidet Buildfähigkeit, lesbaren Bestand und freigegebene Ausführung. Nicht tragbare Schutzverträge führen zur gezielten Einschränkung, nicht zum stillen Rechteabbau. M09 darf beim OS-Umzug weder alte Runtimeidentität noch fehlende spätere Effekte oder heutige Autorität aus der Sicherung erfinden. Die `.factory/`-Dateischreibregel bleibt auch für alternative IPC-Lösungen erhalten.

Diese Präzisierung ist ein Koordinatornachtrag nach dem abgeschlossenen 350er-Review, keine bereits geprüfte Linux-/Windows-Portierung. Ziel-OS, Mindestversionen und zugesagte Profile benötigen einen eigenen Entscheid. WSL ersetzt keinen nativen Windows-Nachweis.

### Stärkere Angreifer oder physische Gefahren

Benötigen einen eigenen Einsatzentscheid mit tatsächlichen OS-, Verwahr- beziehungsweise Teilnehmergrenzen. Fachlich qualifizierte lokale Controller verantworten echte Sicherheitsfristen und Anlagenzustände. Ein Cloud-Task ist keine Ventilsicherung; bloßes Stoppen kann in manchen Anlagen selbst gefährlich sein. Dieser Entwurf erweitert v1 nicht still auf solche Garantien.

## 9. Welche Entscheidungen noch ratifiziert werden müssen

A7 präzisiert vorhandene Ziele, ist aber kein Ersatz für die anerkannten Architekturentscheidungen:

- [ADR 0002](../../adr/0002-daemon-and-api-transport-plugins.md) und [ADR 0007](../../adr/0007-supervised-plugin-process-protocol-v1.md): minimale Start- und Kontrollreserve, echte Gesamtgrenzen, Nurlese-/Fehlerdiagnose bei defektem Kernstand und keine optionale Startpflicht konkretisieren. Die in älteren Socketbeispielen genannten Pfade außerhalb `.factory/` gelten nicht als neue Erlaubnis; die stärkere Projektregel bleibt maßgeblich.
- [ADR 0003](../../adr/0003-logical-api-and-event-sourcing-v1.md): historische Fakten, Inhaltsreferenzen, aktuelles Wissen, späte Ergebnisse und begrenzte Teilnehmerverträge konsistent spezifizieren. Replay bleibt rein und darf keine Plugins, Alarme oder Worker auslösen.
- [ADR 0019](../../adr/0019-restore-and-what-a-recovered-database-may-claim.md): die stärkere A7-Forderung zu unbekannter Snapshot-Nachgeschichte und späterem Widerruf muss ausdrücklich entschieden werden. „Im alten Snapshot kein Versuch“ darf im A7-Ziel nicht pauschal „nie gesendet“ bedeuten. Das ist nicht als heute durchgängig implementiert dargestellt.
- [ADR 0020](../../adr/0020-run-telemetry-and-the-evaluation-bench.md): Aufnahmegrundmenge, Instrumentwechsel, begrenzte Vergleichbarkeit, tatsächlicher Kontextverbrauch und fachliche Abnahme weiter konkretisieren.
- Eigene Betriebs-/Teilnehmerverträge brauchen unabhängige Beobachtung, getrennte legitime Eingriffsautorität, ausdrücklich verwaltete AuthoritySource, autonome lokale Sicherheitsmodi, menschliche Zuständigkeit, Geheimnisverwahrung, rechtskonforme Datenverfügung und tatsächliches Fencing. Wo die Gegenstelle diese Bedingungen nicht trägt, bleibt der engere Vertrag sichtbar.

Version 1 behält den Task als ausgetauschte Arbeitseinheit gemäß [ADR 0010](../../adr/0010-design-baseline-reconciliation.md). Factory-eigene Dateischreibvorgänge bleiben ausschließlich unter `.factory/`. Menschliche `AGENTS.md`, Harnesskonfiguration und bestehende Arbeitsprodukte werden nicht automatisch überschrieben. Geheimnisse bleiben hinter ihrer vorgesehenen Grenze, nicht in Ereignissen, Prompts, Prüfbelegen oder Exporten.

## 10. Wie daraus überprüfbare Umsetzung wird

Der [Prüfplan](pruefplan.md) nennt die zwölf ursprünglichen kombinierten Grenzversuche sowie den additiven OS-Profilversuch P13. [P02 ist inzwischen lokal teilweise ausgeführt](validation/p02/README.md), aber ohne vollständigen Teilnehmer-/Widerrufsnachweis. Dazu kommen die konkreten Gegenprüfungen jedes Szenariozweigs und Residues. Eine vollständige Zuordnung wird zunächst als Dokument überprüft; Implementierung und Zusammenspiel müssen danach gesondert nachgewiesen werden.

Sinnvolle Reihenfolge für die Einordnung in den **bestehenden** ADR- und Backlogprozess:

1. Gemeinsame Fakten-, Identitäts-, Inhalts- und Operationsbegriffe samt Grenzen abstimmen.
2. Einen vertikalen Pfad von Auftrag über Freigabe und Intent bis zum unbekannten Teilnehmerausgang prüfen.
3. Wiederherstellung und aktuelle Autorität einschließlich lebendem Original und spätem Ergebnis ergänzen.
4. Aufnahme-, Zeit-, Kontroll- und Diagnosegrenzen unter kombinierter Last prüfen.
5. Erst dann unabhängige Beobachtung, menschliche Übergabe und zusätzliche Einsatzprofile konkret freigeben.

Das ist keine zweite dauerhafte Implementierungs-Roadmap und keine automatische Priorisierung des vorhandenen Backlogs. Keine realen Nachrichten, Fehlerexperimente, Änderungen an Teilnehmern oder Veröffentlichungen sind durch diesen Entwurf genehmigt.

**Leitsatz:** Factory soll genau sagen können, was noch vorhanden, nutzbar, bekannt und erlaubt ist — und seine Module so verbinden, dass der Ausfall einer Zusatzfähigkeit nicht unnötig die übrigen mitreißt.
