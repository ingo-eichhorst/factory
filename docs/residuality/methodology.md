# Verfahren, Semantik und Evidenzgrenzen

## Mandat

Den Factory-Entwurf aus möglichst vielen Blickwinkeln unter Stress setzen, bis zu 200 konkrete Stressoren sammeln, daraus weniger Residues ableiten, mit einer Inzidenzmatrix iterativ überarbeiten und einen Abschlussbericht liefern. Umgesetzt als 200 Karten in 20 Perspektiven. **„Alle möglichen Perspektiven“ ist ein Suchauftrag, keine erfüllbare Vollständigkeitsgarantie.**

Praktische Annäherung an Barry O’Reillys Residuality Theory: nicht nur bekannte Ausfälle absichern, sondern auch durch ungewöhnliche Veränderungen die Überlebensfähigkeit von Strukturen untersuchen und wiederkehrende Restfähigkeiten komponieren. Keine externe Primärliteraturrecherche, kein Anspruch auf eine autorisierte oder wortgetreue Umsetzung seiner Methode.

## Begriffe

- **Stressor:** eine Veränderung von Umwelt, Nutzung, Ressourcen, Wissen oder Verantwortung, die eine bisher tragende Annahme angreift. Nicht notwendig ein Fehler; Wachstum oder Verkauf des Unternehmens sind ebenfalls Stressoren.
- **Residue:** die noch brauchbare Reststruktur samt verbleibender Fähigkeit und Grenze. „Backup einschalten“ ist eine Maßnahme; „die ohne Originalgerät rekonstruierbare Evidenzmenge“ ist ein Residue-Kandidat. Die Tabellen unterscheiden Reststruktur, Mindestdienst und Verstärkung.
- **Obligation/Kontrolle:** ein konkreter Entwurfsvertrag, den die gewünschte Restfähigkeit unter dem zugeordneten Stressor benötigt. Die 36 Einträge sind nicht 36 neue Komponenten: 32 erhalten in A0–A3 eine Entwurfsantwort, vier bleiben offen.
- **Inzidenzmatrix:** Zuordnung Stressor × Residue. `1` bedeutet direkte relevante Restfähigkeit im Modell; `0` bedeutet nicht direkt zugeordnet, **nicht irrelevant oder geschützt**. „Incident matrix“ wird hier so verstanden. Zusätzlich gibt es zwölf geordnete Incident-Kaskaden.
- **Architekturbewertung:** Liste bereits benannter versus noch fehlender Obligationen pro Stressor/Revision. Kein Risiko-, Reife- oder Zuverlässigkeitsscore.

## Ablauf und Prozessspur

| Runde | Material | Bearbeitung | Erhaltener Stand |
|---|---|---|---|
| Vorarbeit | Erste 18 Karten und sechs grobe Residual-Hypothesen | Bestehende Analyse und ADR-/Codebefunde lesen | [Erste Analyse](../residuality-stressor-analysis.md) bleibt unverändert |
| Ausgang | Architektur-ADRs und tatsächlicher Arbeitsstand | Drei Ebenen trennen: Prototyp / Bibliotheken / Vertrag | [A0](iterations/A0-baseline.md) |
| Runde 1 | S001–S160, 16 Perspektiven | Restfähigkeiten formulieren, 16 Kandidaten und Obligationen zuordnen, Matrix berechnen, Lücken prüfen | [A1](iterations/A1-local-safety.md), 338 Inzidenzen |
| Runde 2 | A1-Lücken plus S161–S180 | Gesellschaftliche Autorität, neue Nutzung und gemeinsame Recoverygrenzen angreifen | [A2](iterations/A2-recovery-authority.md), 387 Inzidenzen |
| Runde 3 | A2-Lücken plus S181–S200 | Extreme Totalverluste und geordnete Kaskaden, Grenzen statt Scheinsicherheit | [A3](iterations/A3-boundaries.md), 443 Inzidenzen |
| Abschluss | Finaler Korpus | Alle vier Stände gegen dieselben 200 Karten auswerten, Prozessdaten prüfen | [Bericht](final-report.md), [Architektur](architecture-A3.md) |

Die Residue-/Kontrollkandidaten wurden aus der Voranalyse und den bekannten Architekturfragen vorbereitet und entlang des Korpus präzisiert. Die Karten stammen vom selben Analysten, sind bewusst gewählt und wurden nicht zufällig oder blind gezogen. S181–S200 sind **Gegenproben**, kein unabhängiger Holdout. Auch die zeitliche Arbeit in mehreren Runden beseitigt nicht den Bestätigungsbias des Autors.

Ein `introduced=A1` bedeutet „in dieser Entwurfsrevision als vollständige Obligation explizit gebündelt“, nicht „diese Idee kam in keinem früheren ADR vor“. A0 rechnet konservativ nur den minimalen bestehenden Vertrag an; vorhandene Teilmechanismen und früher dokumentierte Zielbilder sind nicht weg. Code-Implementierungsreife wird überhaupt nicht aus diesem Feld abgeleitet.

## Kanonische Tabellen

Alle Quelltabellen sind UTF-8-CSV mit Semikolontrennung, Listen innerhalb einer Zelle werden durch Leerzeichen getrennt. Kein Secret und kein kopierter sensitiver Liveinhalt.

| Tabelle | Semantik |
|---|---|
| `perspectives.csv` | 20 Suchperspektiven, Leitfrage, ID-Bereich und Quellenbasis |
| `stressors.csv` | 200 konkrete Situationen, gebrochene Annahme, verbleibende Fähigkeit, benötigte Obligationen, dominanter Restbetriebsmodus und Grenze |
| `residues.csv` | 16 verdichtete Reststrukturen mit Mindestdienst, Verstärkung, Preis, Grenze und architektonischer Heimat |
| `controls.csv` | 36 präzise Obligationen, direkte Residue-Zuordnung, Einführungsrevision und vorgeschlagener Test |
| `residue-dependencies.csv` | 26 bedingte Voraussetzungen zwischen Residues; **kein unbedingter Start-DAG**, Rückbeziehungen können für andere Betriebsphasen gelten |
| `incidents.csv` | zwölf geordnete Mehrfachangriffe, nicht weitere unabhängige Stressor-IDs |
| `experiments.csv` | 17 noch nicht ausgeführte Produkt-/Betriebsexperimente und ihre Falsifikationskriterien |

`surviving_capability` und `response_mode` beschreiben die **angestrebte Residual-Hypothese unter den zugeordneten Verträgen**, nicht eine bereits gemessene Eigenschaft von A0 oder der Implementierung. Der unveränderte Modus in allen vier Bewertungszeilen dient dem Vergleich mit demselben Zielrestbetrieb; bei fehlenden Obligationen ist gerade nicht belegt, dass die frühere Architektur ihn tragen könnte. Bei Verlustkarten kann selbst der Zielrestbetrieb keine Arbeit mehr erhalten.

Pro Karte wurde mindestens ein benötigter Vertrag explizit gewählt. Daraus folgt `B[s,r] = 1`, falls mindestens eine zugeordnete Kontrolle `c` das Residue `r` hat. Das ist eine **kuratierte semantische Zuordnung**, kein NLP-Clustering. Andere relevante Beziehungen können fehlen. `R1 hat mehr Karten als R2` ist daher kein objektives Prioritätsmaß.

## Berechnung und Interpretation

Für Revision A gilt `available(A) = {c | introduced(c) ≤ A}`; `offen` ist in keiner Revision verfügbar. Für Stressor s:

```text
missing(s,A) = required(s) minus available(A)
benannt     = missing ist leer
teilweise   = einige required sind verfügbar, andere fehlen
fehlend     = keine required verfügbar
```

Alle drei Werte sind **Modellaussagen**. Selbst `benannt` kann `verlust`, `grenze` oder `halten` als einzig ehrlichen Restbetrieb ergeben. S190 (alle Information verloren) ist ein gezieltes Gegenbeispiel gegen die Fehlinterpretation als Erfolg.

Die Residue-Überlappung zählt `sum_s B[s,r1] * B[s,r2]`. Sie ist weder statistische Korrelation noch kausale Ausfallwahrscheinlichkeit. Die Diagonale ist die direkte Spaltensumme. Gemeinsame Voraussetzungen werden in der separaten Abhängigkeitstabelle sichtbar; sie sind in B nicht still transitiv eingerechnet.

Die finale Matrix hat 200 × 16 binäre Zellen, 443 davon direkt zugeordnet. Die Architekturbewertung hat 200 × 4 = 800 Zeilen. Die eingeführten Antworten sind additiv; deshalb kann die Zahl fehlender Obligationen durch die Rechenregel nur sinken. **Monotone Verbesserung ist teilweise konstruktionsbedingt und kein empirischer Beweis.** Ob die zusätzlichen Verträge tatsächlich helfen oder neue Risiken erzeugen, prüfen Kaskaden, Trade-offs und spätere Experimente.

## Evidenzklassen

| Klasse | In diesem Paket |
|---|---|
| Quellenbefund | ADR-Vertrag oder konkrete Codefundstelle; kein Nachweis des gesamten Betriebs |
| Entwurfshypothese | Alle Stressor-Wirkungsannahmen, Residues, Zuordnungen und A1–A3 |
| Dokumentenprüfung | 11 ausgeführte Python-Tests für IDs, Referenzen, Berechnung, deterministische Ableitung und negative Modellfälle |
| Empirischer Produktnachweis | **Keiner in diesem Auftrag.** T01–T17 sind vorgeschlagen, nicht bestanden. Keine Factory-Test-/Chaos-/Live-Drills ausgeführt. |

Es gibt keine Häufigkeits-, Wahrscheinlichkeits-, RTO-, Kosten- oder Erfolgszahlen aus Livebetrieb. Testbudgets und RPO/RTO werden nicht nachträglich passend zur Messung erfunden, sondern vor einem genehmigten Drill vereinbart.

## Grenzen und Abbruchkriterium

200 ist die gewünschte Obergrenze, kein mathematischer Sättigungspunkt. Ähnliche physische Ursachen wurden nur getrennt, wenn andere Annahmen oder Restfähigkeiten betroffen sind (z.B. Stromausfall versus Verlust des Schlüssels). Kaskaden sind absichtlich nicht als unabhängige Wahrscheinlichkeiten gezählt. Exakte Textduplikate sind technisch ausgeschlossen, semantische Redundanz kann bleiben.

Die 16 Residues sind gröber als die 200 Karten; deshalb kann eine neue Verpflichtung in ein vorhandenes Residue passen, ohne dass echte Konvergenz vorliegt. R15 ist eine **vertragliche Restfähigkeit** (sicher nicht aufnehmen), keine behauptete laufende Verarbeitung nach kompromittiertem OS. Bei S190 bleibt überhaupt keine rekonstruierbare Arbeit.

Nächster Erkenntnisschritt: unabhängiger Mensch prüft besonders fehlende/überbreite Zuordnungen und gemeinsame Ausfalldomänen; danach isolierte Experimente. Mehr Matrixhäkchen durch immer allgemeinere Residue-Begriffe wäre kein Fortschritt.
