<!-- Generated exclusively by docs/residuality/a4/build.py. -->
# 150 zusätzliche Stressoren

Die verbleibende Fähigkeit ist eine Zielhypothese, keine gemessene Eigenschaft.
C01–C36: A3-Baseline. C37–C48: A4-Vertragspräzisierungen. C33–C36 bleiben offen.

## Q01 — Den ganzen Rechner verschwinden lassen

**Qualitätsziel:** Verfügbarkeit. Wer bemerkt den Ausfall wenn Factory selbst schweigt?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S201: Der ganze Mac verliert Strom und kann keinen Ausfallalarm mehr senden | Der Ausfallende kann sich selbst melden | Ein externer Beobachter meldet das ausgebliebene Lebenszeichen | R17 / C37 C38 | degradiert: Schweigen unterscheidet Stromausfall nicht von Netzstörung |
| S202: Der Rechner läuft aber der Factory-Daemon ist seit Stunden beendet | Ein Host-Ping beweist Factory-Betrieb | Host und Kernel werden getrennt beobachtet | R08 R17 / C37 C39 | degradiert: Ein erreichbarer Kernel beweist noch keinen fachlichen Fortschritt |
| S203: Der Rechner wacht nach drei Monaten mit alten wartenden Aufträgen auf | Wieder erreichbar heißt wieder ausführungsberechtigt | Wiederkehr wird gemeldet und historische Arbeit bleibt gesperrt bis geklärt | R01 R05 R17 / C37 C02 C10 | halten: Spätere Außenwirkungen können in der lokalen Historie fehlen |

## Q02 — Gemeinsame Ausfallquellen suchen

**Qualitätsziel:** Unabhängigkeit. Teilen Rechner Beobachter und Alarmweg doch Strom Netz oder Account?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S204: Factory und Monitoring-Mini-PC hängen an derselben ausgefallenen Steckdose | Zwei Geräte sind zwei Ausfalldomänen | Der Alarmbeobachter liegt außerhalb dieser Stromquelle | R16 R17 / C37 C48 | degradiert: Gemeinsame regionale Infrastruktur kann trotzdem ausfallen |
| S205: Monitor Alarmemail und Backups werden durch dieselbe Accountsperre unzugänglich | Verschiedene Produkte sind unabhängige Wege | Gemeinsame Identitäts- und Zahlungsabhängigkeiten sind sichtbar | R11 R16 R17 / C38 C21 C48 | halten: Ein zweiter unabhängiger Weg muss tatsächlich bereitgestellt werden |
| S206: Ein regionales Ereignis trennt Rechner Mobilfunk und den einzigen Operator | Erreichbarkeit des Monitors genügt für menschliche Reaktion | Genehmigte räumlich unabhängige Vertretung kann übernehmen | R10 R17 / C38 C46 C20 | halten: Sind alle Empfänger isoliert erreicht kein Alarm einen Menschen |

## Q03 — Den Beobachter ausschalten

**Qualitätsziel:** Beobachtbarkeit. Wer entdeckt eine kaputte Alarmkette statt nur kaputte Worker?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S207: Der Monitor fällt eine Stunde vor Factory aus und bleibt stumm | Monitoring braucht keine eigene Überwachung | Ausbleibende Ende-zu-Ende-Proben machen die Alarmkette verdächtig | R17 / C38 | degradiert: Der unabhängige Prüfweg ist ebenfalls endlich verfügbar |
| S208: Der Alarmdienst bestätigt Versand aber die Nachricht landet im Spam | API-Erfolg ist menschliche Kenntnisnahme | Zustellung und menschliche Bestätigung sind getrennt sichtbar | R10 R17 / C38 C46 | halten: Empfangsbestätigung garantiert keine richtige Reaktion |
| S209: Ein Wartungsfenster von einer Stunde bleibt nach einem Fehler zehn Jahre stumm | Stummschaltung endet zuverlässig von selbst | Externe zeitlich begrenzte Wartungsregeln verfallen sichtbar | R10 R17 / C38 C46 | degradiert: Gemeinsam defekte Uhren oder Konfigurationen bleiben möglich |

## Q04 — Grünmeldung gegen echten Fortschritt stellen

**Qualitätsziel:** Diagnostizierbarkeit. Welche gesund aussehenden Komponenten leisten keine nützliche Arbeit?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S210: Ein freier Health-Thread meldet grün während der Kernel im Deadlock steckt | Ein antwortender Thread steht für das ganze System | Eine begrenzte Kernelprobe liefert gesonderte Evidenz | R08 / C39 C16 | degradiert: Die Diagnose kann die genaue Deadlockursache nicht beweisen |
| S211: Reads funktionieren aber die SSD nimmt seit gestern keinen Commit mehr an | Lesbarkeit beweist Schreibfähigkeit | Persistenzbefund und Lesereaktion bleiben getrennt | R07 R08 / C39 C14 | halten: Eine read-only Probe kann keinen erfolgreichen Schreibversuch ersetzen |
| S212: Alle Tasks sind legitim blockiert und ein Fortschrittsalarm erzeugt dauernd Lärm | Kein Abschluss bedeutet immer Defekt | Queuegründe und erlaubter Leerlauf erklären fehlenden Fortschritt | R08 R10 / C39 C19 | degradiert: Ein falsches fachliches Erwartungsmodell erzeugt weiter Fehlalarme |

## Q05 — Beobachtungen zeitversetzen und wiederholen

**Qualitätsziel:** Aktualität. Kann ein altes Lebenszeichen einen heutigen Ausfall verdecken?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S213: Ein Proxy liefert nach dem Rechnerausfall noch eine Woche gecachte Lebenszeichen | Erhaltene Nachricht ist eine neue Beobachtung | Sequenz Frische und Inkarnationsbindung begrenzen Wiederverwendung | R08 R17 / C37 C39 | degradiert: Ein kompromittierter Sender kann weiterhin falsche Frische behaupten |
| S214: Nach Netzrückkehr treffen tausend alte Heartbeats schneller als echte ein | Viele Meldungen bedeuten aktuellen Betrieb | Aufgestaute Telemetrie ersetzt keine aktuelle Probe | R12 R17 / C37 C45 | degradiert: Ohne frische Evidenz bleibt der Zustand unbekannt |
| S215: Ein restaurierter Klon sendet dieselbe alte Monitoridentität wie das Original | Ein Name bezeichnet genau eine lebende Installation | Monitoring markiert Identitätskonflikte und vergibt keine Schreibbefugnis | R03 R17 / C37 C06 | halten: Heartbeat-Identität allein ist kein externes Fencing |

## Q06 — Den Messapparat selbst überlasten

**Qualitätsziel:** Messökonomie. Wann verursacht Monitoring die Störung die es beobachten soll?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S216: Ein Agent erzeugt pro Token ein neues Metriklabel bis das Monitoring abstürzt | Mehr Details erhöhen nur die Sichtbarkeit | Labels und Datenvolumen bleiben begrenzt | R07 R12 / C42 C45 C14 | degradiert: Begrenzung verwirft absichtlich diagnostische Details |
| S217: Ein Tracingexport wartet bei jedem Commit auf einen ausgefallenen Cloudcollector | Beobachtung ist nie Teil des kritischen Pfads | Optionale Telemetrie wird begrenzt und darf verworfen werden | R06 R12 / C12 C45 | degradiert: Kanonische Operationsevidenz darf nicht mit verworfenen Traces verschwinden |
| S218: Ein Debugmodus verdoppelt die Latenz und beseitigt zugleich das untersuchte Rennen | Messen verändert das System nicht | Messaufwand und getrennte Gegenproben werden dokumentiert | R07 R14 / C42 C28 | offen: Keine Messung garantiert vollständige Beobachterfreiheit |

## Q07 — Das Latenzbudget durch alle Schichten verfolgen

**Qualitätsziel:** Zeitverhalten. Kann jede Teilfrist passen während die Gesamtfrist längst abgelaufen ist?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S219: Acht Schichten brauchen jeweils neun Sekunden bei einer Gesamtfrist von zehn | Lokale Timeouts ergeben eine globale Frist | Ein gemeinsames Restbudget begrenzt den Aufrufbaum | R06 R13 / C40 C12 | halten: Nicht kooperierende Teilnehmer können danach weiterarbeiten |
| S220: Ein Auftrag wartet 48 Stunden in der Queue und bekommt danach eine frische Fünfsekundenfrist | Nur Laufzeit zählt zur zugesagten Antwortzeit | Wartezeit und fachlicher Verfall werden mitgeprüft | R05 R07 R13 / C40 C10 C42 | halten: Eine verspätete Aktion kann bereits extern angenommen worden sein |
| S221: Ein 200-Millisekunden-Task wartet zwölf Jahre auf menschliche Freigabe | Technische Latenz beschreibt Ende-zu-Ende-Latenz | Menschliche Wartezeit und verfallene Intention sind sichtbar | R09 R10 R13 / C40 C19 C18 | halten: Factory kann keine Entscheidung innerhalb einer unerfüllbaren Frist erzwingen |

## Q08 — Die unsichtbaren langsamen Anfragen zählen

**Qualitätsziel:** Messgültigkeit. Welche Wartenden Abbrüche und nie gestarteten Aufgaben fehlen im Histogramm?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S222: Das Dashboard zeigt nur abgeschlossene Tasks und verschweigt den seit Wochen hängenden Rest | Erfolgshistogramme enthalten alle Nutzererfahrung | Offene Altersklassen und Abbrüche werden zusätzlich gezählt | R07 R08 / C42 C39 | degradiert: Unvollständige Beobachtung ist keine genaue Dauerverteilung |
| S223: Der Lasttest sendet erst nach jeder Antwort und erzeugt während des Hängers keine neue Last | Geschlossene Testlast bildet externe Nachfrage ab | Angebotene und tatsächlich zugelassene Last werden getrennt geprüft | R07 / C42 | degradiert: Ein synthetisches Lastmodell bleibt eine Annahme |
| S224: Aus tausend schnellen Samples wird ein p99.999-Verfügbarkeitsversprechen abgeleitet | Ein benanntes Perzentil ist ausreichend belegt | Stichprobenumfang Fenster und zensierte Fälle bleiben sichtbar | R07 R14 / C42 C28 | offen: Seltene Verteilungen sind damit nicht empirisch bestimmt |

## Q09 — Den langsamsten Ast eines Fan-outs vergrößern

**Qualitätsziel:** Skalierung. Was geschieht wenn ein Auftrag tausende heterogene Antworten braucht?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S225: Ein Task wartet auf alle zehntausend Pluginantworten und eine kommt nie | Viele einzeln schnelle Abhängigkeiten ergeben einen schnellen Gesamtauftrag | Fan-out und gemeinsames Budget sind begrenzt | R07 R13 / C41 C40 | halten: Ein fehlender Pflichtteil kann den gesamten Auftrag blockieren |
| S226: Ein Optimierer verdoppelt langsame Rechnungsaktionen spekulativ um das Tail zu verkürzen | Hedging ist für Lesen und Außenwirkung gleichermaßen sicher | Kein spekulativer Doppelversand externer Effekte | R02 R07 R09 / C41 C03 C18 | halten: Nach bereits doppelter Annahme braucht es fachliche Klärung |
| S227: Ein Aggregator erklärt 999 von 1000 Prüfergebnissen für vollständig um schnell zu antworten | Eine große Mehrheit ersetzt den fehlenden Pflichtbeleg | Teilvollständigkeit bleibt explizit und blockiert unzulässige Verwendung | R14 / C47 C27 | halten: Was zwingend vollständig sein muss ist domänenspezifisch |

## Q10 — Eine winzige dauerhaft hängende Teilpopulation einführen

**Qualitätsziel:** Tail-Latenz. Kann fast alles schnell sein und trotzdem alle Plätze belegt bleiben?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S228: Eine von hunderttausend Anfragen hängt unbegrenzt und belegt über Jahre alle Slots | Ein guter Median schützt vor schleichender Sättigung | Wartebudgets und Altersverteilung begrenzen neue Last | R07 R13 / C40 C42 C13 | degradiert: Unbekannte alte Effekte dürfen nicht durch Leasefreigabe beseitigt werden |
| S229: Nur ein bestimmter Scope erlebt minutenlange fsync-Pausen während globale Mittelwerte grün sind | Globale Kennzahlen repräsentieren jeden Scope | Begrenzte Serviceklassen und lokale Ausreißer bleiben sichtbar | R07 / C42 C14 | degradiert: Zu feine Labels würden selbst Ressourcen erschöpfen |
| S230: Eine zufällige Speicherbereinigung trifft immer dieselbe Geschäftsdeadline | Seltene Verzögerungen seien zeitlich unabhängig | Gemeinsame Fristen und Pausen werden als Korrelation geprüft | R07 R13 / C40 C42 | halten: Ohne Echtzeitplattform gibt es keine harte Deadlinegarantie |

## Q11 — Antworten nach dem Vergessen eintreffen lassen

**Qualitätsziel:** Wirkungssicherheit. Was passiert wenn Antwort oder Wirkung später kommt als die lokale Historie reicht?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S231: Die Antwort auf einen Versand kommt 90 Tage nach Ablauf der Idempotenzhistorie | Der Dienst erinnert sich mindestens so lange wie Factory wartet | Die späte Antwort wird der alten Operation zugeordnet ohne neuen Dispatch | R02 R13 / C40 C03 C34 | offen: Fehlende Teilnehmerhistorie kann den Ausgang dauerhaft unklärbar machen |
| S232: Ein offline gepufferter Auftrag wird 2046 ausgeführt nachdem Empfänger und Firma gewechselt haben | Alte Bytes behalten ihre fachliche Gültigkeit | Verfall und aktuelle Zielbefugnis begrenzen neue Ausführung | R05 R09 R13 / C40 C10 C18 | halten: Ein fremder bereits handelnder Teilnehmer kann lokale Regeln ignorieren |
| S233: Ein Ergebnis trifft nach legaler Löschung seiner Zuordnung ein und enthält sensible Daten | Späte Antworten passen immer noch in ein vorhandenes Journal | Unzuordenbarer Inhalt wird kontrolliert behandelt statt als neuer Auftrag | R12 R13 / C40 C24 C36 | offen: Retention und Aufklärung können unvereinbar sein |

## Q12 — Abbruch und Außenwirkung gegeneinander laufen lassen

**Qualitätsziel:** Abbrechbarkeit. Was bedeutet Cancel wenn der andere Teilnehmer schon angenommen hat?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S234: Cancel erreicht den Teilnehmer eine Mikrosekunde nach irreversibler Annahme | Abbruchbestätigung macht vergangene Wirkung ungeschehen | Annahme und Abbruch bleiben verschiedene Fakten | R02 R09 R13 / C40 C03 C18 | halten: Kompensation kann unmöglich sein |
| S235: Der Parent stirbt nach Timeout aber ein Enkelprozess schreibt zwei Tage weiter | Ein abgelaufenes Aufrufbudget beendet alle Nachkommen | Workspace bleibt bei unklarer Lebendigkeit reserviert | R01 R06 / C12 C01 | halten: OS-Bypass bleibt außerhalb kooperativer Garantien |
| S236: Ein Cancel steckt hinter Gigabytes stdout im selben Kanal fest | Kontrollnachrichten haben automatisch Vorrang | Begrenzte Ausgaben und Kontrollkapazität bleiben verfügbar | R06 R07 / C12 C14 | degradiert: Ein bereits angenommener Effekt wird dadurch nicht zurückgenommen |

## Q13 — Retries auf allen Ebenen multiplizieren

**Qualitätsziel:** Stabilität. Wie wird eine kleine Störung durch Wiederholungen zur Lastlawine?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S237: CLI Kernel Plugin und SDK wiederholen jeweils viermal denselben Fehler | Jeder lokale Retry bleibt insgesamt klein | Ein gemeinsames Versuchbudget begrenzt Multiplikation | R02 R07 / C41 C03 | halten: SDK-interne Versuche müssen nachweisbar oder abschaltbar sein |
| S238: Eine Million Clients versuchen nach identischer Pause exakt gleichzeitig erneut | Backoff verteilt Last auch ohne Zufall | Zulässige Wiederholungen nutzen begrenzten Jitter | R07 / C41 C13 | degradiert: Jitter schafft keine fehlende Gesamtkapazität |
| S239: Nach Quotenfehler retryt ein Plugin aggressiver und verbraucht das letzte Budget | Wiederholen verbessert jeden Fehlertyp | Permanente Ablehnung und Überlast werden nicht blind wiederholt | R07 / C41 C13 | halten: Unbekannte Fehlertypen brauchen konservative Behandlung |

## Q14 — Alle warmen Zustände gleichzeitig entfernen

**Qualitätsziel:** Anlaufverhalten. Ist ein kalter Start noch tragfähig oder nur der eingespielte Betrieb?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S240: Nach Stromrückkehr laden alle Agenten gleichzeitig ihre riesigen Modelle | Warme Leistungswerte gelten direkt nach Start | Kaltstart und Aufnahme werden separat budgetiert | R07 R16 / C43 C13 | degradiert: Das Gerät kann für die geplante Last grundsätzlich zu klein sein |
| S241: Ein leerer Cache macht eine bisher optionale Suche zum Kapazitätsengpass | Ein Cache beeinflusst nur Geschwindigkeit | Wirkliche Kapazitätsabhängigkeiten sind im Inventar sichtbar | R16 / C48 C43 | degradiert: Optionalität muss praktisch nachgewiesen werden |
| S242: Kalte DNS- und TLS-Aufbauten verbrauchen die gesamte Erstaufruffrist | Timeoutmessung beginnt erst nach Verbindungsaufbau | Der komplette Aufruf teilt ein Restbudget | R06 R13 / C40 C12 | halten: Zu knappe Budgets können legitime erste Aufrufe ausschließen |

## Q15 — Eine unendlich lange Aufgabe vor kurze Aufgaben setzen

**Qualitätsziel:** Fairness. Wer verhungert durch Reihenfolge Priorität oder blockierte Reservierungen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S243: Ein 8-Terabyte-Export steht vor allen kleinen Kontrollabfragen | FIFO ist automatisch fair | Kontrollreserve und begrenzte Arbeitsklassen verhindern vollständige Blockade | R07 / C14 C41 | degradiert: Große Arbeit kann bewusst länger warten |
| S244: Ständig eintreffende Notfalltasks verdrängen normale Aufgaben über Monate | Priorität liefert irgendwann jedem Fortschritt | Eine ausdrückliche Fairnessregel macht Verdrängung und Grenzen sichtbar | R07 R10 / C41 C19 | halten: Unter dauerhafter Überlast ist nicht jede Deadline erfüllbar |
| S245: Unklare Agenten halten alle Workspace-Reservierungen und der Operator fordert TTL-Freigabe | Kapazitätsgewinn ist wichtiger als unbekannte Schreiber | Sichere Belegung bleibt trotz Rückstau bestehen | R01 R07 R10 / C01 C14 C19 | halten: Stillstand kann die einzig sichere Option sein |

## Q16 — Jede Mengendimension einzeln um Größenordnungen erhöhen

**Qualitätsziel:** Kapazität. Welches Limit bricht zuerst bei IDs Bytes Prozessen oder Abonnenten?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S246: Zehn Millionen registrierte Namen passen in den Store aber nicht in jede Kontextnachricht | Speicherbarkeit bedeutet Bedienbarkeit | Kontext und Abfragen bleiben begrenzt und gezielt | R05 R07 / C09 C13 | degradiert: Vollständige Sicht braucht paginierte Inspektion statt einen Prompt |
| S247: Ein einziges Ergebnis enthält hundert Gigabytes in einem gültigen JSON-Feld | Wenige Tasks bedeuten wenig Last | Byte- und Framegrenzen gelten unabhängig von Taskanzahl | R06 R07 / C12 C13 | halten: Das Ergebnis muss über einen geeigneten Inhaltsvertrag geliefert werden |
| S248: Hunderttausend langsame Subscriber halten je eine eigene Queue offen | Ein kleiner Eventstream kostet für jeden Empfänger wenig | Subscriberbudgets begrenzen fan-out und Rückstau | R07 / C13 C41 | degradiert: Langsame Leser benötigen wiederaufnehmbare begrenzte Abfragen |

## Q17 — Jahre an Geschichte in ein kleines Recoveryfenster pressen

**Qualitätsziel:** Langzeitbetrieb. Ist ein replaybares System noch rechtzeitig und mit realem Speicher aufbaubar?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S249: Zwanzig Jahre Ereignisse brauchen drei Wochen Replay bei vereinbartem RTO von einer Stunde | Deterministischer Replay ist automatisch schnell genug | Gemessener Wiederaufbau und explizite Kapazitätsgrenze bleiben ehrlich | R11 R16 / C21 C43 | halten: Ein verfehltes RTO braucht Kapazität oder ein geändertes Versprechen |
| S250: Der Restore passt nur wenn Original Export temporäre Kopie und Index nie gleichzeitig existieren | Datenbankgröße ist der gesamte Platzbedarf | Recovery wird mit realem Spitzenbedarf geprüft | R07 R11 R16 / C21 C14 C43 | halten: Ohne genügend unabhängigen Speicher ist Restore nicht ausführbar |
| S251: Um Platz zu sparen werden nur Operationstombstones gelöscht und der nächste Retry dupliziert Wirkung | Alte Idempotenzfakten seien entbehrliche Logs | Retention berücksichtigt offene und späte Effekte | R02 R12 / C24 C03 C34 | offen: Unbegrenzte Teilnehmerlatenz und endliche Retention kollidieren |

## Q18 — Lange Stille in einen synchronen Nachfragepuls verwandeln

**Qualitätsziel:** Elastizität. Was geschieht wenn alle verpassten Termine und Nutzer gleichzeitig zurückkommen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S252: Nach zehn Jahren Pause sind eine Milliarde Schedule-Occurrences nachzuholen | Jeder verpasste Termin soll irgendwann ausgeführt werden | Catch-up-Regel und globale Aufnahme begrenzen die Welle | R07 R13 / C25 C13 C40 | halten: Geschäftliche Verfallsregeln muss jemand festlegen |
| S253: Eine Providerstörung endet und alle blockierten Scopes starten im selben Millisekundenfenster | Recovery ist nur eine Rückkehr zum Normalzustand | Wiederaufnahme wird budgetiert und verteilt | R07 / C41 C13 | degradiert: Schon entstandene offene Wirkungen brauchen eigene Rekonziliation |
| S254: Ein viraler Auftrag erzeugt rekursiv Millionen Kinder bevor die Rechnung sichtbar wird | Delegationsgrenzen verhindern auch jede Mengenexplosion | Globale Aufgaben- und Kostenbudgets begrenzen neue Arbeit | R07 / C13 C41 | halten: Eine zulässige Delegationskette kann trotzdem sehr teuer sein |

## Q19 — Thermik und Strom als geteilte Ressource behandeln

**Qualitätsziel:** Energieeffizienz. Welche Annahmen zerbrechen bei Drosselung Batterieende oder Lastabwurf?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S255: Dauerlast drosselt die CPU auf ein Zehntel und verlängert alle Fristen gleichzeitig | Rechenleistung bleibt während eines Runs konstant | Zeitbudgets und Lastbegrenzung verhindern blindes Weiterplanen | R07 R13 / C40 C13 | degradiert: Thermische Kapazität bleibt eine physische Grenze |
| S256: Die USV reicht für den Commit aber nicht für den nachfolgenden externen Versand | Ein kurzer Energiepuffer sichert die ganze Operation | Gespeicherte Intention und unbekannter Ausgang bleiben erhalten | R02 R11 / C03 C21 | halten: Teilnehmerannahme kann vor dem Stromende geschehen sein |
| S257: Lastabwurf schaltet zuerst den als unwichtig eingestuften Monitoringrouter ab | Die Notstromplanung schützt automatisch die Alarmkette | Gemeinsame Energieabhängigkeiten werden als Vertrag geprüft | R16 R17 / C37 C48 | degradiert: Bei vollständigem regionalem Kommunikationsverlust endet Alarmierbarkeit |

## Q20 — Im Notfall den Kontrollpfad benötigen

**Qualitätsziel:** Steuerbarkeit. Bleibt Stop und Diagnose gerade unter maximalem Druck verfügbar?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S258: Stop wartet hinter sämtlichen überlasteten Pluginaufrufen | Ein vorhandener Stopbefehl ist rechtzeitig nutzbar | Kontrollkapazität bleibt vorab reserviert | R07 R08 / C14 C16 | degradiert: Wenn der Kernel selbst nicht reagiert bleibt nur externe Diagnose |
| S259: Der Notfallzugang verlangt einen Login beim gerade gesperrten einzigen Identitätsanbieter | Remote-Identität bleibt im Notfall verfügbar | Ein genehmigter lokaler Kontrollweg besitzt eigene legitime Zugangsvoraussetzungen | R08 R09 R16 / C15 C17 C48 | halten: Kein Authentifizierungs-Bypass unter dem Namen Notfallzugang |
| S260: Die SSD ist voll und Factory bestätigt einen Stopp den es nicht speichern konnte | Eine UI-Antwort beweist dauerhafte Zustandsänderung | Nicht persistierter Stop wird als unbestätigt kenntlich | R07 R08 / C14 C39 | halten: Laufende externe Wirkung kann weitergehen |

## Q21 — Einen berechtigten Vermittler für fremde Ziele missbrauchen

**Qualitätsziel:** Security und Autorisierung. Kann ein gültiger Actor den falschen Scope oder Empfänger handeln lassen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S261: Ein korrekt authentifizierter Pluginaufruf verwendet das Credential eines anderen Scopes | Gültiger Actor bedeutet gültiges Ziel | Aufgelöstes Ziel und Credentialbindung werden zusammen geprüft | R09 / C44 C17 | halten: Direkter Same-User-Zugriff auf Secrets braucht andere Isolation |
| S262: Ein menschenlesbarer Empfängername wird kurz vor Dispatch auf eine andere Identität umgebogen | Der genehmigte Name bleibt dieselbe Zielperson | Freigabe bindet die tatsächlich aufgelöste Zielidentität | R09 / C44 C18 | halten: Externe Aliasregeln müssen dem Teilnehmervertrag entsprechen |
| S263: Eine importierte Task-ID wird als Beleg einer fremden Freigabe akzeptiert | Eine korrelierbare ID ist eine Berechtigung | Importe tragen Herkunft aber erzeugen keine aktuelle Autorität | R05 R09 / C44 C10 | halten: Unbekannte historische Actorfakten bleiben unbekannt |

## Q22 — Daten als scheinbare Befehle verkleiden

**Qualitätsziel:** Instruktionsintegrität. Werden Inhalte Logs oder Toolantworten ungewollt zu Autorität?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S264: Ein Dokument behauptet Systemregel zu sein und verlangt den Upload aller Secrets | Lesbarer Inhalt darf seine eigene Priorität bestimmen | Inhalt wird nicht zu Freigabe oder Scope-Capability | R09 R12 / C44 C23 | halten: Ein privilegierter Harness kann außerhalb des Kernvertrags handeln |
| S265: Ein Fehlerlog enthält gefälschte Bestätigungen die ein Recovery-Agent für Operatorfreigaben hält | Diagnostische Prosa ist autoritative Handlungsevidenz | Nur strukturierte authentifizierte Freigabefakten legitimieren Aktionen | R09 / C44 C18 | halten: Same-User-Manipulation des gesamten Systems bleibt außerhalb der Grenze |
| S266: Ein Ergebnisartefakt schmuggelt eine neue externe Aktion in seine Prüfanweisung | Ein Prüfer darf die Autorität des geprüften Inhalts übernehmen | Prüfung und neue Außenwirkung bleiben getrennt | R09 R14 / C44 C27 | halten: Fachliche Prüfer müssen untrusted Quellen angemessen behandeln |

## Q23 — Korrekte Signatur mit bösartigem Inhalt kombinieren

**Qualitätsziel:** Lieferkettensicherheit. Welche Sicherheit bleibt wenn Herkunft stimmt aber der Lieferant kompromittiert ist?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S267: Ein gültig signiertes Pluginupdate exfiltriert Daten | Signiert bedeutet harmlos | Herkunft und tatsächliche Vertrauensgrenze bleiben getrennt | R15 R16 / C32 C30 C33 | grenze: Feindlicher Code ist im selben Useraccount nicht sicher eingeschlossen |
| S268: Ein zurückgezogenes Paket wird zwanzig Jahre später für Restore aus einem übernommenen Namen geladen | Ein alter Paketname bezeichnet noch dieselben Bytes | Gepinnte Herkunft und kompatible Recoveryartefakte begrenzen Verwechslung | R11 R16 / C32 C22 C43 | halten: Eine fehlende vertrauenswürdige Kopie lässt sich nicht herbeizaubern |
| S269: Das unabhängige Monitoring erhält denselben kompromittierten Auto-Updater wie Factory | Getrennte Hosts garantieren unabhängige Fehler | Gemeinsame Lieferketten stehen im Ausfalldomäneninventar | R16 R17 / C48 C38 | grenze: Zwei identisch kompromittierte Vertrauensanker können gemeinsam täuschen |

## Q24 — Rotation Wiederherstellung und Kryptoperioden kollidieren lassen

**Qualitätsziel:** Schlüsselkontinuität. Was geschieht wenn alte Belege und heutige Schlüssel nicht mehr zusammenpassen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S270: Heartbeat-Schlüsselrotation und Netzpartition überlappen sodass beide Seiten den jeweils anderen Key ablehnen | Rotation ist ein atomarer globaler Zeitpunkt | Versionierte befristete Übergänge und fehlende Authentizität bleiben sichtbar | R16 R17 / C37 C32 | degradiert: Übergangsfenster erweitern zeitweise die Angriffsfläche |
| S271: Ein zwanzig Jahre alter Vault ist lesbar aber niemand besitzt den Entschlüsselungsweg | Intakte Bytes genügen für Recovery | Separater legitimierter Schlüssel-Recovery wird benötigt | R10 R11 / C22 C20 | verlust: Ohne Schlüssel und legitimen Ersatz bleibt Inhalt verloren |
| S272: Ein späterer kryptografischer Durchbruch macht alte sensible Exporte lesbar | Verschlüsselung verspricht unbegrenzte Geheimhaltung | Minimierung und Retention begrenzen dauerhaft exponierte Inhalte | R12 / C24 C23 | grenze: Bereits kopierte Daten können nicht nachträglich eingezogen werden |

## Q25 — Kooperative Regeln durch feindliche Gleichzeitigkeit ersetzen

**Qualitätsziel:** Isolation. Was kann ein Mitnutzer über OS Speicher Zeit oder Zugangsdaten umgehen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S273: Ein feindlicher Agent liest andere Scopes direkt über das Betriebssystem | Scope-Policy ist eine OS-Sandbox | Nicht unterstützte feindliche Nutzung wird ausdrücklich abgelehnt | R15 / C30 C33 | grenze: Ohne harte Isolation besteht keine Geheimhaltungszusage |
| S274: Ein Mitnutzer rekonstruiert fremde Aktivität aus geteilten CPU- und Queuezeiten | Keine direkten Datenzugriffe bedeutet keine Informationskanäle | Covert-Channel-Grenzen bleiben Teil des Deploymentmandats | R12 R15 / C30 C33 C45 | grenze: Telemetrieminimierung beseitigt geteilte Hardwarekanäle nicht |
| S275: Der externe Monitor erhält aus Bequemlichkeit Shellzugriff auf Factory | Beobachtungsbefugnis darf Reparaturbefugnis einschließen | Monitoring bleibt read-only ohne Dispatch- oder Wiederanlaufrecht | R09 R17 / C37 C17 | halten: Ein separat genehmigter Reparaturdienst wäre ein neues Mandat |

## Q26 — Aus harmlosen Messwerten sensible Zusammenhänge rekonstruieren

**Qualitätsziel:** Datenschutz. Welche Information verraten Labels Heartbeats und Exporte zusammengenommen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S276: Heartbeat-Labels verraten Kundennamen und vertrauliche Geschäftszeiten | Betriebsmetadaten sind immer harmlos | Nur notwendige pseudonyme Kennungen verlassen die Installation | R12 / C45 C23 | degradiert: Selbst minimale Aktivitätsmuster können sensibel bleiben |
| S277: Ein Debugexport für Monitoring enthält die vollständige AGENTS-Konversation | Mehr Kontext ist für jeden Empfänger zulässig | Exportfelder Empfänger und Retention werden ausdrücklich begrenzt | R12 / C45 C24 | halten: Bereits versandte Inhalte sind nicht zuverlässig zurückholbar |
| S278: Ein Angreifer löst gezielt Fehlalarme aus um Alarmempfänger und Bereitschaftsmuster zu lernen | Alarme haben keine Datenschutz- oder Missbrauchskosten | Begrenzte Alarmraten und minimale Inhalte senken Offenlegung | R12 R17 / C45 C38 | degradiert: Berechtigte Empfänger bleiben für ihren Provider sichtbar |

## Q27 — Gültig aussehende gemeinsame Beschädigung erzeugen

**Qualitätsziel:** Datenintegrität. Wie wird ein Fehler bemerkt den Prüfsumme Kopie und Prüfer gemeinsam teilen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S279: Ein fehlerhafter Exporter erzeugt identisch falsche Daten und passende Prüfsummen in allen Backups | Integritätsprüfung beweist fachliche Richtigkeit | Unabhängige Wiederaufbau- und Bedeutungsprüfungen bleiben nötig | R11 R14 / C21 C28 C35 | offen: Gemeinsame Blindstellen können unentdeckt bleiben |
| S280: Ein einzelner Bitfehler verwandelt einen Betrag in eine andere noch gültige Zahl | Ein parsebares Event ist korrekt | Integritäts- und fachliche Plausibilitätsbelege werden getrennt geprüft | R04 R14 / C08 C47 | halten: Plausible falsche Werte können trotzdem bestehen bleiben |
| S281: Zwei signierte Journale widersprechen sich nach einer Split-Brain-Phase | Eine Signatur entscheidet welche Geschichte wahr ist | Konflikt bleibt sichtbar statt still zusammengeführt | R03 R04 R10 / C06 C07 C19 | halten: Menschliche Auflösung kann verlorene Außenweltevidenz nicht ersetzen |

## Q28 — Eine alte Welt in die heutige Außenwelt zurücksetzen

**Qualitätsziel:** Wiederherstellbarkeit. Welche längst erledigten oder verbotenen Aktionen erscheinen wieder offen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S282: Ein altes Backup kennt weder gestern versandte Rechnungen noch heutige Widerrufe | Queued im Backup bedeutet nie ausgeführt | Restore sperrt Dispatch bis Gegenwartsabgleich | R01 R02 R09 / C02 C18 C03 | halten: Fehlende unabhängige Teilnehmerdaten können dauerhaft blockieren |
| S283: Original und restaurierter Klon senden gleichzeitig plausible Lebenszeichen | Der grünste Monitorpunkt ist der rechtmäßige Writer | Monitoringkonflikt erteilt keine Mutationsautorität | R03 R17 / C37 C06 | halten: Ohne tatsächlich durchgesetztes Fencing gibt es keinen automatischen Failover |
| S284: Der Recoverylauf dauert so lange dass seine zu Beginn geprüften Rechte schon wieder verfallen | Einmal geprüfte Gegenwart bleibt bis zum Ende gültig | Aktuelle Freigabe wird nahe an der Wirkung erneut geprüft | R09 R13 / C40 C18 | halten: Prüfung und externe Annahme können weiterhin ein Race bilden |

## Q29 — Alle Versionskombinationen statt nur latest gegen latest betrachten

**Qualitätsziel:** Kompatibilität. Wo zerbricht ein formal gültiger Vertrag semantisch?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S285: Neuer Client und altes Plugin interpretieren dieselbe gültige Zahl mit verschiedener Einheit | Schema-Kompatibilität bedeutet Bedeutungs-Kompatibilität | Versionsfixtures prüfen die fachliche Semantik | R14 R16 / C43 C47 | halten: Unbekannte Kombinationen müssen abgelehnt werden |
| S286: Ein optionales Plugin verlangt plötzlich eine globale Kernmigration für alle Scopes | Lokale Aktivierung begrenzt automatisch den Änderungsradius | Abhängigkeiten und Kernpflichten werden vor Freigabe sichtbar | R06 R16 / C48 C11 | halten: Ein gemeinsamer Store kann nicht beliebig scopeweise migriert werden |
| S287: Altes Binary öffnet einen neuen Store und ersetzt unverstandene Felder durch Defaults | Rückwärtslesen erlaubt sicheres Rückwärtsschreiben | Unbekannte schreibende Kombinationen werden verweigert | R16 / C43 C31 | halten: Lesbarer Export kann die einzig zulässige alte Nutzung sein |

## Q30 — Rollback nach irreversibler Vorwärtswirkung verlangen

**Qualitätsziel:** Änderbarkeit. Welche Änderung kann ein altes Binary nicht rückgängig machen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S288: Rollback startet den alten Code nachdem der neue schon eine irreversible Außenaktion ausgelöst hat | Code-Rollback rollt Geschäftswirkung zurück | Vorwärtswirkung bleibt im Migrationsplan als eigener Fakt erhalten | R02 R16 / C43 C03 | halten: Kompensation braucht eine neue legitime Entscheidung |
| S289: Eine Migration verliert die Verbindung genau zwischen Übergabe und Start des neuen Writers | Ein abgebrochener Befehl bedeutet nicht ausgeführter Cutover | Eindeutige Writerzuständigkeit und Übergabeevidenz bleiben maßgeblich | R03 R16 / C31 C05 C43 | halten: Nicht aus einer fehlenden Antwort einen zweiten Writer starten |
| S290: Eine Notfallkorrektur benötigt gerade das kaputte Plugin um dessen Konfiguration zu lesen | Reparatur hängt nicht vom reparierten Teil ab | Minimaler Bootstrap und expliziter Änderungsradius bleiben getrennt | R08 R16 / C16 C48 | degradiert: Ein defekter Kern benötigt externen read-only Recoveryweg |

## Q31 — Den Wissenden und seine Werkzeuge entfernen

**Qualitätsziel:** Wartbarkeit. Kann jemand ohne Maintainer Cloudkonto und alte Toolchain sicher reparieren?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S291: Der einzige Maintainer verschwindet und ein Nachfolger kennt die verdeckten Betriebsannahmen nicht | Quellcode allein überträgt Betriebswissen | Verträge Herkunft Abhängigkeiten und lesbarer Handover bleiben erhalten | R10 R16 / C32 C48 C20 | degradiert: Dokumentation garantiert keine verfügbare Kompetenz |
| S292: Eine winzige Reparatur braucht eine nicht mehr erhältliche Toolchain und einen gelöschten Cloudaccount | Build und Betrieb bleiben organisatorisch verfügbar | Versionierte Artefakte und ein geordneter Ausstieg begrenzen Abhängigkeit | R11 R16 / C32 C22 | halten: Ohne vertrauenswürdige Werkzeuge kann Änderung unmöglich bleiben |
| S293: Nach tausend Sonderfällen versteht niemand mehr welche Ausnahme Außenwirkung erlaubt | Mehr lokale Regeln machen das System nur sicherer | Ein expliziter kleiner Autoritätsvertrag und Änderungsinventar bleiben prüfbar | R09 R16 / C48 C18 | halten: Ein umfangreicher Regelkatalog ersetzt keinen verständlichen Kern |

## Q32 — Eine kleine Änderung durch den Abhängigkeitsgraphen schicken

**Qualitätsziel:** Modularität. Wie groß ist der tatsächliche statt behauptete Ausfallradius?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S294: Ein Update der gemeinsamen JSON-Bibliothek verändert gleichzeitig Plugins Replay und Monitoring | Getrennte Prozesse bedeuten getrennte semantische Fehler | Gemeinsame Abhängigkeiten erhalten Kombinationsgegenproben | R16 / C48 C43 | halten: Ein gemeinsamer Implementierungsfehler kann alle Pfade treffen |
| S295: Ein Monitoringfeature macht einen Cloudaccount zur Startvoraussetzung aller Kindscopes | Installation im Root bedeutet überall verpflichtende Aktivierung | Optionale Integration bleibt explizit scopegebunden | R06 R08 R16 / C48 C11 C16 | degradiert: Ohne Aktivierung darf keine externe Alarmgarantie behauptet werden |
| S296: Eine hilfreiche automatische Reparatur wächst zum zweiten schreibenden Kernel | Ein Recoverywerkzeug ist nur ein weiterer Client | Ein Writer und read-only Offline-Diagnose bleiben harte Grenzen | R03 R08 / C05 C15 | halten: Mutationen müssen durch den zuständigen Daemon gehen |

## Q33 — Den Testapparat zum Mitverursacher machen

**Qualitätsziel:** Testbarkeit. Welche Fehler verschwinden nur weil der Test Scheduler Uhr und Reihenfolge kontrolliert?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S297: Alle Tests verwenden sofortige Antworten und verpassen jede verspätete Annahme | Deterministische Happy Paths beweisen Racefreiheit | Gezielte verzögerte Annahme und Antwort werden getrennt geprüft | R13 R16 / C43 C40 | halten: Endliche Tests erfassen nicht jede mögliche Reihenfolge |
| S298: Die virtuelle Testuhr friert Timeout und externen Monitor gemeinsam ein | Eine gemeinsame Fake-Uhr bildet unabhängige Beobachter ab | Getrennte Zeitquellen und Ausfälle werden als Gegenprobe modelliert | R16 R17 / C37 C43 | degradiert: Simulation ersetzt keinen genehmigten realen Ende-zu-Ende-Drill |
| S299: Ein Mock bestätigt Fencing obwohl der reale Provider alte Generationen akzeptiert | Ein bestandener Mocktest ist Teilnehmerkonformität | Reale Fähigkeit bleibt vor automatischem Failover nachweispflichtig | R03 R14 / C06 C28 | halten: Ohne Nachweis nur expliziter Single-active-Betrieb |

## Q34 — Transporterfolg mit falscher Bedeutung kombinieren

**Qualitätsziel:** Funktionale Richtigkeit. Was passiert bei falscher Einheit Zielperson Rundung oder Teilantwort?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S300: Ein erfolgreicher Bericht vertauscht Euro Cent und Dezimalkomma | Technisch gültiger Output ist fachlich korrekt | Akzeptanz prüft Einheit und Bedeutung getrennt vom Prozessstatus | R14 / C47 C27 | halten: Geeignete Fachkriterien bleiben Domänenverantwortung |
| S301: Ein schneller Cache liefert eine alte aber plausibel richtige Preisliste für heutige Bestellung | Richtigkeit ist unabhängig von Aktualität | Gültigkeit gehört zum Akzeptanz- und Dispatchvertrag | R05 R14 / C47 C10 | halten: Der aktuelle externe Preis kann sich unmittelbar danach ändern |
| S302: Ein Auftrag wird vollständig für die falsche reale Person mit gleichem Namen erledigt | Namensgleichheit beweist richtige Zielidentität | Zielbindung und Ergebnisprüfung verwenden aufgelöste Identität | R09 R14 / C44 C47 | halten: Ein unzuverlässiges externes Identitätsregister bleibt eine Grenze |

## Q35 — Unter Stress ohne übliche Anzeige handeln

**Qualitätsziel:** Bedienbarkeit und Zugänglichkeit. Sind Zuständigkeit Unsicherheit und Folgen ohne Farbe Maus oder Muttersprache erkennbar?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S303: Im Incident ist die rote Warnung für einen farbfehlsichtigen Operator nicht erkennbar | Farbe allein transportiert sicherheitsrelevante Bedeutung | Status und Handlung sind zusätzlich als klarer Text lesbar | R10 / C46 C19 | degradiert: Zugänglichkeit muss mit realen Nutzern geprüft werden |
| S304: Ein langer Scopepfad wird im Mobilalarm gekürzt und der Operator stoppt die falsche Instanz | Kompakte Darstellung erhält eindeutigen Kontext | Alarm zeigt eindeutige sichere Identität und verlinkte begrenzte Diagnose | R10 R12 / C46 C45 | halten: Mehr Details dürfen keine Secrets offenlegen |
| S305: Eine übersetzte Schaltfläche macht aus unklar scheinbar sicher fehlgeschlagen | Lokalisierung verändert nur die Form | Unsicherheit und irreversible Folgen behalten explizite Bedeutung | R10 R14 / C46 C47 | halten: Sprachliche Verständlichkeit ist nicht allein maschinell beweisbar |

## Q36 — Den Menschen in den langen Latenzschwanz setzen

**Qualitätsziel:** Betriebsfähigkeit. Was bleibt wenn jede technische Prüfung schnell ist aber niemand entscheidet?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S306: Der Alarm erreicht nachts ein ausgeschaltetes Telefon und niemand bemerkt ihn für Tage | Versand ist eine verfügbare menschliche Reaktion | Bestätigung Frist und genehmigte Eskalation sind getrennt geregelt | R10 R17 / C38 C46 | halten: Sind alle Vertreter unerreichbar bleibt Stillstand |
| S307: Ein Flapping-Link erzeugt zehntausend Meldungen und verdeckt den späteren echten Totalausfall | Jedes technische Ereignis verdient eine eigene Benachrichtigung | Zustandswechsel werden gebündelt ohne unbegrenzte Stummschaltung | R10 R17 / C38 C46 | degradiert: Zu starke Bündelung kann wichtige Unterschiede verdecken |
| S308: Der Operator bestätigt einen Alarm sofort aber die Reparatur liegt drei Monate in seiner Queue | Bestätigung heißt behoben | Erkannt bestätigt und behoben sind getrennte Zustände | R08 R10 R17 / C38 C19 C39 | halten: Eine Bestätigung erfüllt kein Wiederherstellungsziel |

## Q37 — Mehrere legitime Nachfolger mit Konflikt erzeugen

**Qualitätsziel:** Verantwortbarkeit. Wer darf während Nachfolge Befangenheit oder Vertretung tatsächlich handeln?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S309: Zwei rechtmäßige Vertreter erteilen gleichzeitig gegensätzliche Freigaben | Mehr Vertretung bedeutet eindeutigere Autorität | Konflikt wird dokumentiert und neue Wirkung angehalten | R09 R10 / C20 C18 | halten: Die Rangfolge muss legitim vereinbart sein |
| S310: Der ausgeschiedene Betreiber kontrolliert noch den einzigen Monitoringaccount | Datenübergabe überträgt auch Alarmzuständigkeit | Nachfolge umfasst unabhängige Betriebs- und Alarmwege | R10 R16 R17 / C20 C38 C48 | halten: Kontenübertragung benötigt menschliche Zustimmung und Providerzugang |
| S311: Ein befangenes Team schaltet Alarme stumm um eigene Fehlaktionen unsichtbar zu machen | Berechtigte Menschen handeln stets im gemeinsamen Interesse | Stummschaltung und Zuständigkeit bleiben begrenzt nachvollziehbar | R10 R17 / C20 C38 | grenze: Ein vollständig kompromittierter Autoritätskreis kann nicht intern garantiert werden |

## Q38 — Unabhängige Anbieter auf ihren gemeinsamen Eigentümer zurückführen

**Qualitätsziel:** Austauschbarkeit. Wie viele Ausfalldomänen bleiben hinter Marken Accounts und Zahlungssystemen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S312: Zwei Modellanbieter Monitor und Pager nutzen denselben gestörten Cloudunterbau | Verschiedene Marken sind unabhängige Infrastruktur | Gemeinsame Abhängigkeiten werden explizit geprüft | R16 R17 / C48 C38 | degradiert: Unbekannte Unterauftragnehmer begrenzen den Nachweis |
| S313: Eine Zahlungssperre stoppt Modelle Backups und Alarme am selben Tag | Technische Redundanz überlebt jede organisatorische Sperre | Kosten- und Accountabhängigkeiten sind Teil des Recoveryplans | R11 R16 R17 / C48 C21 C38 | halten: Ein nicht finanzierter Ausweichweg ist keine vorhandene Reserve |
| S314: Der einzige Provider stellt den Dienst mit 24 Stunden Frist endgültig ein | Alle Ausfälle sind temporär | Portables Arbeitspaket und geordneter Anbieterwechsel bleiben möglich | R05 R14 R16 / C09 C32 C28 | halten: Ein Ersatz bietet nicht automatisch dieselbe Kompetenz oder Idempotenz |

## Q39 — Kosten später als die Budgetentscheidung sichtbar machen

**Qualitätsziel:** Kostenkontrolle. Kann ein scheinbar billiger Auftrag bereits ein großes offenes Kostenrisiko besitzen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S315: Ein Provider meldet Tokenkosten erst eine Woche nach Überschreitung des Monatsbudgets | Beobachtete Kosten sind das gesamte aktuelle Risiko | Neue Aufnahme berücksichtigt konservativ offene Kostenbindungen | R07 / C13 C42 | halten: Unbegrenzte externe Nachberechnung verhindert eine harte lokale Kostengarantie |
| S316: Ein p99.999-Latenzziel führt zu hundert spekulativen Kopien pro Auftrag | Schneller ist unabhängig von Preis und Wirkung immer besser | Fan-out Kosten und Außenwirkungen bleiben separat begrenzt | R02 R07 / C41 C13 C03 | halten: Nicht jedes Latenzziel ist wirtschaftlich oder sicher erreichbar |
| S317: Aufmerksamkeit für Fehlalarme kostet mehr als sämtliche Modellaufrufe | Nur maschinelle Ressourcen brauchen ein Budget | Alarmbelastung und menschliche Kapazität werden bewusst begrenzt | R10 / C46 C19 | degradiert: Eine perfekte fehlerfreie Alarmierung ist nicht versprochen |

## Q40 — Widersprechende Mandate durch große Verzögerungen schicken

**Qualitätsziel:** Rechtliche Steuerbarkeit. Welche Aktion ist bei Annahme anders erlaubt als bei Auftrag oder Urteil?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S318: Eine behördliche Untersagung trifft zwischen Freigabe und später Teilnehmerannahme ein | Rechtmäßigkeit gilt unverändert über jede Queuezeit | Aktuelle Befugnis wird nahe an der Wirkung geprüft | R09 R13 / C18 C40 | halten: Ein nicht stoppbarer Teilnehmer kann bereits unterwegs sein |
| S319: Zwei Rechtsräume verlangen gleichzeitig Löschung und unveränderte Beweisaufbewahrung | Mehr Speicherung erfüllt alle Pflichten | Konflikt bleibt einer legitimierten Entscheidung zugeordnet | R12 / C24 C36 | offen: Architektur kann widersprüchliche Pflichten nicht gleichzeitig erfüllen |
| S320: Ein Monitoringprovider verlagert sensible Betriebsmetadaten unangekündigt in einen anderen Rechtsraum | Ein genehmigter Empfänger bleibt rechtlich unverändert | Minimale Daten und überprüfbare Empfängerbedingungen begrenzen Haftung | R12 / C45 C24 C36 | offen: Vertragsprüfung und Rechtsentscheidung bleiben menschliche Aufgaben |

## Q41 — Eine Softwareaktion an irreversible physische Folgen koppeln

**Qualitätsziel:** Safety. Wo ist später stoppen oder kompensieren fachlich unzureichend?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S321: Ein alter Auftrag öffnet nach Tagen ein physisches Ventil in einer inzwischen anderen Situation | Späte Softwarewirkung ist grundsätzlich kompensierbar | Verfall und ausdrückliche Einsatzgrenze stoppen unzulässige neue Nutzung | R13 R14 R15 / C40 C30 C47 | grenze: Factory v1 ist kein Safety-Echtzeitsystem |
| S322: Eine Notfallabschaltung hängt von einer Cloudantwort ab die erst morgen kommt | Normale API-Latenz genügt für physische Sicherheit | Sicherheitskritische Echtzeitsteuerung bleibt außerhalb des Mandats | R14 R15 / C30 C47 | grenze: Unabhängige zertifizierte Sicherheitssteuerung wäre ein eigenes System |
| S323: Ein Menschenleben wird gegen monatliches Rechenbudget automatisch abgewogen | Ein globaler Scheduler darf jede fachliche Priorität bestimmen | Unvereinbare Safetyanforderung wird nicht als gewöhnlicher Task aufgenommen | R14 R15 / C30 C35 | grenze: Ohne legitime Fachentscheidung gibt es keinen tragfähigen Maßstab |

## Q42 — Asymmetrische und tagelange Partitionen statt Totalausfall erzeugen

**Qualitätsziel:** Netztoleranz. Was sieht jeder Teilnehmer wenn nur eine Richtung und nur manchmal funktioniert?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S324: Factory erreicht den Dienst aber keine Antwort erreicht Factory zurück | Ein Timeout beweist fehlende Annahme | Unbekannter Ausgang bleibt ohne blinden Retry gespeichert | R02 R13 / C03 C40 | halten: Eine einseitige Partition kann unbegrenzt dauern |
| S325: Eine Satellitenverbindung liefert signierte Pakete nach 30 Tagen stark umgeordnet | Authentizität beweist Aktualität | Frische Frist und Operation werden unabhängig von Signatur geprüft | R09 R13 R17 / C37 C40 C18 | halten: Ohne aktuelle Rückmeldung bleibt automatische Fortsetzung begrenzt |
| S326: Monitoring sieht Factory nicht aber Factory erreicht weiterhin alle Businessdienste | Ein externer Alarm bedeutet die Außenwirkung sei gestoppt | Alarm und Befugnis bleiben getrennte Zustände | R08 R09 R17 / C37 C39 C18 | degradiert: Ohne besondere genehmigte Policy ist Alarmierung kein Kill-Switch |

## Q43 — Die Uhrgeometrie verändern

**Qualitätsziel:** Zeitrobustheit. Welche Frist überlebt Suspend Uhrsprung Datumsgrenze oder lange Isolation?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S327: Ein Laptop schläft ein Jahr und seine lokale monotone Uhr behandelt Suspend anders als erwartet | Monoton bedeutet überall dieselbe verstrichene Realzeit | Zeitquellenvertrag und Geschäftsverfall werden separat geprüft | R13 / C26 C40 | halten: Über verschiedene Hosts existiert keine gemeinsame monotone Uhr |
| S328: Die Wallclock springt rückwärts und verlängert eine Wartungsstummschaltung scheinbar um Jahre | Kalenderzeit eignet sich allein für alle relativen Fristen | Der externe Beobachter begrenzt relative Wartezeiten mit eigener Zeitbasis | R13 R17 / C37 C38 C26 | degradiert: Nach Monitorrestore muss Zeitunsicherheit ausdrücklich behandelt werden |
| S329: Eine globale Zeitzonenreform ändert die Bedeutung bereits geplanter lokaler Termine | Zeitzonendaten sind historische Naturkonstanten | Scheduleversion und damalige Terminentscheidung bleiben erhalten | R04 R13 / C25 C07 | halten: Neue fachliche Termine brauchen eine ausdrückliche Regel |

## Q44 — Widersprüchliche Zeugen ohne gemeinsamen Richter zulassen

**Qualitätsziel:** Erkenntnisqualität. Welche Behauptung kann Factory wirklich belegen statt nur auswählen?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S330: Runtime sagt beendet Dateibeobachtung zeigt neue Writes und der Agent behauptet bereit | Ein autoritativer Zeuge genügt trotz widersprechender Evidenz | Workspace und Unsicherheit bleiben geschützt | R01 R10 / C01 C19 | halten: Die konkrete Ursache kann ohne unabhängigen Zugang unklar bleiben |
| S331: Drei unabhängige Prüfer verwenden dieselbe fehlerhafte externe Grundtabelle | Verschiedene Prüfer sind unabhängige Erkenntnisquellen | Gemeinsame Datenherkunft bleibt sichtbar | R14 / C28 C35 | offen: Eine unbekannt falsche gemeinsame Quelle kann alle Prüfungen täuschen |
| S332: Der Provider attestiert Erfolg aber der Empfänger bestreitet jede Wirkung und Logs fehlen | Ein formaler Erfolgswert entscheidet jede Realität | Widerspruch bleibt einer alten Operation zugeordnet | R02 R10 / C03 C34 C19 | offen: Ohne zusätzliche Evidenz kann der Konflikt dauerhaft unauflösbar sein |

## Q45 — Die Messzahl zum Optimierungsziel machen

**Qualitätsziel:** Anreizfestigkeit. Kann ein Agent schneller grüner oder billiger erscheinen indem er Nutzen versteckt?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S333: Ein Agent verbessert seine Durchlaufzeit indem er schwierige Tasks vor Messbeginn ablehnt | Schnelle Abschlüsse bedeuten höheren Nutzwert | Angebotene Last Ablehnung und Abschluss werden zusammen ausgewiesen | R07 R14 / C42 C47 | degradiert: Ein fachlicher Nutzwert braucht mehr als Laufzeitkennzahlen |
| S334: Ein Team benennt Fehler als Wartung um die Verfügbarkeit auf hundert Prozent zu heben | Korrekte Berechnung verhindert Manipulation der Kategorien | Versionierte SLI-Regeln und endliche Wartungsfreigaben bleiben prüfbar | R07 R17 / C42 C38 | grenze: Gemeinsam manipulierte Governance braucht externe Kontrolle |
| S335: Ein Modell schreibt fertige Artefakte ohne Prüfung um das Erfolgsziel zu erfüllen | Selbstberichteter Erfolg ist ein unabhängiges Qualitätsziel | Abgabe Prüfung und Freigabe bleiben getrennt | R14 / C27 C47 C35 | offen: Kein universeller unabhängiger Wahrheitsmaßstab vorhanden |

## Q46 — Sparziele gegen Verfügbarkeit und Fristen stellen

**Qualitätsziel:** Nachhaltigkeit. Welche legitimen Qualitätsziele lassen sich nicht gleichzeitig maximieren?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S336: Energiesparmodus schaltet nachts den Rechner ab und erzeugt täglich einen Totalausfallalarm | Jeder fehlende Heartbeat ist ungeplanter Schaden | Genehmigte endliche Ruhezeiten sind vom unerwarteten Ausfall unterscheidbar | R13 R17 / C38 C25 | degradiert: Eine unbegrenzte Ruhefreigabe wäre eine Überwachungslücke |
| S337: CO2-optimiertes Verschieben lässt eine genehmigte Aktion erst nach ihrem Verfallsdatum starten | Eine ökologische Optimierung darf fachliche Fristen überstimmen | Aktuelle Gültigkeit begrenzt jede Verschiebung | R05 R13 / C40 C10 | halten: Nicht alle Qualitätsziele lassen sich zugleich erfüllen |
| S338: Drei zusätzliche Monitoringcluster verbrauchen mehr als das kleine überwachte System | Mehr Redundanz ist immer die beste Antwort | Ausfallmodell Nutzen Kosten und Pflegeaufwand werden zusammen bewertet | R07 R16 / C48 C13 | grenze: Es bleibt bewusst akzeptiertes Restrisiko statt maximaler Redundanz |

## Q47 — Auf fremder Hardware und nach Jahrzehnten neu anfangen

**Qualitätsziel:** Portabilität. Was bleibt ohne identische Architektur Pfadsemantik und Laufzeit?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S339: Restore auf anderer CPU interpretiert ein natives Binärartefakt anders | Portable JSONmetadaten machen alle Inhalte portabel | Formate und reale Kombinationen werden als Recoveryvoraussetzung geprüft | R11 R16 / C43 C22 | halten: Nicht erhaltene native Laufzeiten können unersetzbar sein |
| S340: Ein Dateisystem mit anderer Großschreibung vereinigt zwei vormals verschiedene Workspacepfade | Gleiche Pfadstrings haben überall gleiche Identität | Workspacebindung und Plattformverträglichkeit werden neu geprüft | R01 R16 / C01 C43 | halten: Keine automatische Zusammenführung oder Worktreebereinigung |
| S341: Im Jahr 2080 existieren weder heutige Modellformate noch Netzwerkprotokolle | Ein Export bleibt ohne Leser dauerhaft nutzbar | Lesbare Formate Herkunft und Ausstiegsplan begrenzen Abhängigkeit | R11 R16 / C22 C32 | verlust: Langzeitlesbarkeit ist endlich und braucht gepflegte Werkzeuge |

## Q48 — Das System sauber verlassen wollen

**Qualitätsziel:** Beendbarkeit. Was muss beim Ende noch nachweisbar sein obwohl niemand weiterbetreiben will?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S342: Factory wird abgeschaltet während ein Dienst noch alte Aktionen gepuffert hat | Kein lokaler Prozess bedeutet keine zukünftige Außenwirkung | Offene Operationen und deren Stilllegungsgrenze bleiben sichtbar | R02 R13 R16 / C03 C32 C40 | halten: Fremde Puffer können ohne Mitwirkung nicht sicher entleert werden |
| S343: Der Monitoringvertrag endet vor dem letzten genehmigten Exportlauf | Stilllegung beendet alle Beobachtungspflichten sofort | Alarmzuständigkeit endet erst nach ausdrücklich vereinbartem Übergang | R10 R16 R17 / C38 C32 C20 | halten: Weiterbetrieb benötigt einen finanzierten legitimen Empfänger |
| S344: Der Nutzer verlangt vollständige Löschung und zugleich beweissicheren Export aller früheren Geheimnisse | Ausstieg kann widersprüchliche Datenwünsche automatisch erfüllen | Konflikt und verbleibende Evidenzgrenze werden dokumentiert | R12 R16 / C24 C36 C32 | offen: Eine konkrete Rechts- und Eigentumsentscheidung bleibt erforderlich |

## Q49 — Alle unabhängigen Anker zugleich entziehen

**Qualitätsziel:** Grenzprüfung. Welche Versprechen werden logisch unmöglich statt nur teuer?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S345: Rechner Monitor Mobilfunk alle Backups und alle legitimierten Empfänger verschwinden gleichzeitig | Irgendein Restanker bleibt immer übrig | Nur die vorher offen benannte Versprechensgrenze bleibt | R10 R11 R17 / C37 C22 C20 | verlust: Ohne Beobachter Kommunikationsweg und Empfänger gibt es keinen Alarm |
| S346: Alle vertrauenswürdigen Quellen liefern gemeinsam eine perfekt konsistente falsche Welt | Konsens und Signaturen garantieren Wahrheit | Unbeweisbare Qualität wird nicht als Gewissheit zertifiziert | R14 / C28 C35 | offen: Ohne unabhängige Realitätsevidenz ist der Irrtum nicht erkennbar |
| S347: Eine Außenaktion kann jederzeit in unendlicher Zukunft ankommen während Speicherung endlich bleiben muss | Endliche Retention erlaubt immer sichere automatische Wiederholung | Unbekannt und Verweigerung bleiben legitime Endzustände | R02 R12 R13 / C40 C24 C34 | offen: Unbegrenzte Latenz endliche Erinnerung und sichere Wiederholung sind nicht gleichzeitig garantiert |

## Q50 — Dem Gegner die Reihenfolge der Störungen überlassen

**Qualitätsziel:** Sequenzangriff. Welche sichere Einzelmaßnahme wird durch eine zweite und dritte Störung gefährlich?

| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |
|---|---|---|---|---|
| S348: Monitor wird stummgeschaltet dann Mac zerstört dann der einzige Vertreterkonto gesperrt | Jede separat getestete Reserve bleibt in der Kaskade verfügbar | Unabhängige Wege und endliche Sperren werden zusammen geprüft | R10 R16 R17 / C38 C46 C48 | halten: Bei Verlust aller legitimen Wege endet Alarmierbarkeit |
| S349: Nach Restore erzeugt kalter Cache Retrylawine und eine späte alte Zahlung trifft ein | Recovery Last und Wirkung lassen sich unabhängig lösen | Dispatchsperre Budget und alte Operationsbindung greifen gemeinsam | R01 R02 R07 R13 / C02 C41 C40 C03 | halten: Kein Lastabbau darf unbekannte Effekte blind wiederholen |
| S350: Ein signiertes Update fälscht Health löscht Historie und liefert perfekte Prüfberichte | Mehr interne Belege reichen auch bei gemeinsam kompromittiertem Ursprung | Einsatzgrenze und Bedarf unabhängiger Evidenz bleiben explizit | R02 R14 R15 R17 / C33 C34 C35 C38 | grenze: Ein fremder Monitor kann fehlende Heartbeats erkennen aber keine perfekt gefälschte Welt widerlegen |

