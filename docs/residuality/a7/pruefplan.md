# A7 — wie wir den modularen Entwurf prüfen müssen

**Die kombinierten Versuche sind Vorschläge, nicht vollständig ausgeführte Produktversuche.** Für [P02 liegt inzwischen eine partielle lokale Bibliotheksprüfung vor](validation/p02/README.md): vier Rust-Tests bestanden, aber keine echte Teilnehmer-/Widerrufsprüfung; die vorbereitete Mutationsgegenprobe brach wegen veränderter Quellen vor Ausführung ab. P02 insgesamt bleibt offen. Die Nachweisideen jedes Residues und jedes Szenariozweigs stehen zusätzlich im [vollständigen Katalog](generated/stressoren.md). Der bisherige Experimentbestand T01–T29 bleibt historische Grundlage, nicht nachträglich als bestanden markiert.

## Drei verschiedene Prüfungen

1. **Dokumentprüfung:** Stimmen 350 IDs, Zweigarten, Referenzen und Modulzuordnungen? Das lässt sich automatisch prüfen, beweist aber keine Schutzwirkung.
2. **Vertragsprüfung:** Eine isolierte Implementierung oder Gegenstelle muss den behaupteten Schutz tatsächlich einhalten. Dabei muss der Versuch auch erkennen können, dass der Schutz fehlt.
3. **Zusammenspiel:** Mehrere gleichzeitig auftretende Störungen dürfen die Voraussetzungen der jeweils anderen Schutzstruktur nicht unbemerkt aufheben. Erst hier zeigt sich, ob die Module zusammen einen brauchbaren Dienst erhalten.

## Gemeinsame Prüffälle für Modulgrenzen

| Versuch | Kombination | Was erhalten bleiben soll | Woran ein Scheinerfolg erkennbar wäre |
|---|---|---|---|
| P01 | Commitfehler + voller Datenträger + paralleler Zustandsleser | ehrlich begrenzte Diagnose ohne neue behauptete erfolgreiche Mutation | ein grüner Lesezugriff wird zur Schreibfähigkeitszusage |
| P02 | Antwortverlust + alte Sicherung + zwischenzeitlich widerrufene Erlaubnis | unbekannter Ausgang und ungeklärte aktuelle Befugnis bleiben getrennt sichtbar | fehlender alter Versuch wird als Nichtausführung behandelt |
| P03 | Klon + lebendes Original + verlorener Monitorweg | keine unbelegte Übernahme und tatsächlicher Ausschluss alter Schreiber am Teilnehmer | Testdouble bestätigt Ausschluss, realer Teilnehmer akzeptiert beide |
| P04 | Lastwelle + gestaffelte Wiederholungen + kalter Start + Stoppanforderung | begrenzte Gesamtarbeit, erhaltene Identitäten und erreichbarer Steuerweg | einzelne Versuchslimits passen, die Summe wächst trotzdem |
| P05 | Zeitverschiebung + Suspend + abgelaufene Erlaubnis + späte Antwort | getrennte Wartefrist, Handlungsberechtigung und Zuordnung später Evidenz | neuer Timer erlaubt eine alte Handlung erneut |
| P06 | Wartung + Fehlalarme + fehlende Vertretung + echter Hostausfall | erhaltener offener Störungsfall, ehrliche Zustelllage, keine erfundene Reparatur | Stummschalten löscht den Vorfall oder Quittieren gilt als Behebung |
| P07 | Falscher Export + identische Prüfergrundlage + verlorener alter Decoder | Herkunft und fachlicher Widerspruch bleiben nutzbar, sofern unabhängige Grundlage vorhanden | drei Prüfsummen werden als drei unabhängige Wahrheiten gezählt |
| P08 | Manipulierte Messung + ausgelassene schwierige Arbeit + unklarer Preis | begrenzte überprüfbare Erfolgsaussage und sichtbare Messlücken | nicht gemessene Fälle werden als schnell, kostenlos oder erfolgreich eingesetzt |
| P09 | Schädliches Update + gemeinsamer Aktualisierer + falsche Wiederanlaufbelege | tatsächlich unabhängige Prüf-/Wiederanlaufgrundlage innerhalb erklärter Vertrauensgrenze | ein zweiter Prozess unter derselben Angreifermacht gilt als unabhängig |
| P10 | Fehlender Maintainer + verlorenes Konto + alter lesbarer Quelltext | nachgewiesener legitimer Bau-/Betriebsweg oder ehrliche Stilllegung | vorhandene Bytes werden mit vorhandener Reparaturfähigkeit verwechselt |
| P11 | Regelkonflikt + widerrufene Zuständigkeit + irreversible Außenwirkung | begründete Grenze, getrennte Evidenz und keine technische Erfindung rechtlicher Autorität | Besitz eines Passworts wird zur legitimen Nachfolge erklärt |
| P12 | Vollständiger Verlust aller benötigten Quellen oder Alarmwege | ausdrücklich negative Zusage für die betroffene Fähigkeit | ein erfundener Beobachter oder eine nicht vorhandene Kopie rettet den Test |

Die Situationen sind zu konkretisieren: Startzustand, erlaubte Eingänge, Fehlerstelle, Dauer, vorhandene Kopien, gültige Befugnisse, beobachtbare Ergebnisse und Abbruchkriterium. Begriffe wie „unabhängig“ oder „dauerhaft gespeichert“ brauchen jeweils eine überprüfbare Grenze.

## P13 — weiteres Betriebssystem

Der additive [Stressor S351](addenda/S351-weiteres-betriebssystem.md) ergänzt einen vorgeschlagenen Plattformversuch: gleicher Domänenvertrag auf einer ausdrücklich benannten OS-/Dateisystem-/Rechte-/Runtimekombination. Native Pfadidentität und Leases, lokaler Kanalzugriff, Prozessnachkommen, Suspend, Sicherungsübernahme und Secretzugang getrennt prüfen. Cross-Compile ist kein Nachweis gleicher Garantien. P02 auf jedem unterstützten Profil wiederholen. P13 wurde nicht ausgeführt; der ursprüngliche 350er-Review wird dadurch nicht umgeschrieben.

## Attraktion nicht durch geänderte Eingaben vortäuschen

Bei Last- und menschlichen Rückkopplungen zuerst den auslösenden Fehler entfernen und Grundnachfrage, Kapazität und Regeln konstant halten. Danach gesondert Nachfrage oder Regeln ändern. Verschiedene Startzustände unter denselben Bedingungen vergleichen. Endliches Abarbeiten, dauernd von außen eingespeiste Last und Wachstum bis zum Zusammenbruch sind nicht dasselbe wie ein anziehender wiederkehrender Zustand.

Bei Wissens- und Organisationskreisläufen muss das tatsächlich wiederverwendete Ergebnis oder die veränderte Entscheidung beobachtbar sein. Ein falsch positiver Test allein zeigt noch keine Vertrauensverstärkung. Legitime Änderungen von Regeln und transparente Auswahl schwieriger Arbeit bleiben Gegenbeispiele.

## Vor realem Betrieb zu entscheiden

- Gewünschte Betriebsdauer ohne Menschen und erreichbare legitime Vertretung.
- Vertretbare Datenlücke und Wiederanlaufzeit für konkret benannte Inhalte.
- Globale Ressourcen-, Kosten-, Speicher- und menschliche Aufmerksamkeitsgrenzen.
- Anbieterabhängigkeiten, tatsächliche Teilnehmergarantien und deren Aufbewahrungsdauer.
- Datenschutz, Aufbewahrung, Geheimnisverwaltung und erlaubte externe Beobachtung.
- Vertrauensgrenze: kooperative Prozesse unter einem Benutzer oder ausdrücklich stärker isolierter Einsatz.

Kein Zeitplan und keine positive Dokumentprüfung genehmigt von sich aus Alarmversand, Datenexport, Reparatur, Ersatzstart oder Geschäftsaktionen.
