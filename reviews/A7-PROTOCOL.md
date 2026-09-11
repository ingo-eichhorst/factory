# A7 — alle Szenarien zu Zuständen, Residues und Architektur führen

Koordinator: `3f3815e1-38d6-445e-97d3-bbebe92751f3`.
Dies ist die ausdrücklich beauftragte Folgephase nach der abgeschlossenen A6-Entdeckung, nicht Phase 1 des alten Protokolls.

## Auftrag und Grenzen

Analysiere jedes Szenario deines bisherigen Bereichs. Lies deine bisherigen Szenarien, Zustände und Trajektorien sowie `../../docs/residuality/a6/state-qualifications.csv`, `supplemental-branches.csv` und `../../reviews/skeptic/cross-review.md` (Pfade vom eigenen Reviewverzeichnis aus). Alte Zahlen 17/19/14 sind KEINE Zielzahlen. Lies keine neuen A7-Ergebnisse anderer Analysten vor Abschluss deiner Abgabe. Koordinator synthetisiert gemeinsame Module erst danach.

Schreibe ausschließlich in deinem eigenen neuen Unterverzeichnis `a7/`. Originalberichte/CSV-Dateien bleiben unverändert. Keine Produktänderung, Instanzkonfiguration, Agentenverwaltung, externen Aktionen, Livefehler, ADR-Ratifizierung oder Worktrees. Aufgabenworkflow: run_get, run_progress, Entscheidungen, run_complete mit Artefakten bzw. spezifischer Blocker. Ergebnisse in einfachem Deutsch, keine Geheimnisse.

## Analyse

Für JEDE zugeteilte Stressorkarte konkrete bedingte Verläufe und Restfähigkeiten herleiten. Nicht nur die 19 vorher ausdrücklich benannten Attraktorhypothesen behandeln. Es dürfen weitere Hypothesen entstehen, aber nur mit benannter Rückkopplung und Bedingungen. Nicht alles ist ein Attraktor: vorübergehende Störung, äußere Dauerlast, politisch/technisch erzwungener Halt, unbekannter Ausgang, Abschluss und Verlust unterscheiden.

Bei bisher offenen Fällen mindestens die fehlende entscheidende Annahme benennen und bedingte Alternativen entwickeln, wo sinnvoll. Keine erfundene eindeutige Konvergenz. Bei Totalverlust oder fehlender erforderlicher Information ist `keines` ein ehrliches Ergebnis für die betroffene Fähigkeit; keine imaginäre externe Kopie oder perfekte Prüfquelle hinzufügen. Verlust einer Eigenschaft bedeutet nicht automatisch Verlust des ganzen Systems.

Residue = konkret noch nutzbare Struktur/Fähigkeit, NICHT bloß Gegenmaßnahme oder Schadenszustand. Ein Entwurf darf eine Schutzstruktur voraussetzen, muss dann sagen, wodurch sie überlebt, wer sie noch nutzen kann und wann auch sie ausfällt. Solche Kandidaten sind nicht als heute implementiert oder empirisch bestätigt auszugeben. Eine Wiederverwendung muss denselben nutzbaren Gegenstand und dessen Vertragsgrenze treffen; keine pauschale Sammelbox `Resilienz`, `Sicherheit`, `Qualität`.

## Abgabe: drei neue Dateien

### 1. `a7/residues.csv`
Semikolongetrennt, UTF-8, mit korrekter CSV-Quotierung. Spalten:
`id;name;object;survival;prerequisites;failure_boundary;module_hint;architectural_change;validation`

Verwende lokal eindeutige IDs: operations `OPR001...`, governance `GVR001...`, observability `OBR001...`, boundaries `BDR001...`. Anzahl ist frei. Jeder Kandidat muss tatsächlich in deinen Fällen vorkommen. `module_hint` benennt eine begrenzte fachliche Verantwortung, keinen verpflichtenden neuen Dienst. `architectural_change` beschreibt, was Factory dafür bauen/konkretisieren müsste; `validation` einen unterscheidenden Nachweis, nicht bloß Schema-Prüfung.

### 2. `a7/cases.json`
JSON-Liste, genau ein Objekt je zugeteiltem Stressor. Schema:

```json
[
  {
    "stressor_id": "S001",
    "source_states": ["OP002", "OP014"],
    "branches": [
      {
        "state": "Einfacher konkreter Zustandsname",
        "kind": "halt",
        "conditions": "Was für genau diesen Zweig gelten muss",
        "persistence": "Warum er andauert oder wieder endet; bei Attraktorhypothese die tatsächlichen Rückkopplungskanten",
        "residue_ids": ["OPR001"],
        "residue_status": "bedingt",
        "reason": "Was unter dieser Störung warum nutzbar bleibt; keine Umsetzung behaupten",
        "validation": "Welche Beobachtung diesen Verlauf oder diese Erhaltung widerlegen würde"
      }
    ],
    "architectural_consequence": "Szenariospezifische konkrete Änderung und ihre Grenze, kein allgemeiner Werbesatz"
  }
]
```

`source_states`: bekannte Zustands-IDs aus deinem historischen Report, auch leere Liste zulässig bei wirklich offener alter Einordnung. Keine erfundenen historischen IDs. Neue Zustandsformulierungen stehen in `branches[].state` und bekommen vom Koordinator automatisch nachvollziehbare IDs pro Szenario/Zweig.

`kind` ausschließlich: `attraktorhypothese`, `transient`, `extern-erzwungen`, `halt`, `ungewissheit`, `abschluss`, `verlust`, `eskalation`, `offen`.

`residue_status` ausschließlich: `bedingt`, `teilweise`, `keines`, `ungeklaert`.
Bei `keines` oder `ungeklaert` darf `residue_ids` leer sein, aber Grund und Architekturgrenze müssen ausdrücklich ausgefüllt sein. Bei `bedingt`/`teilweise` mindestens ein tatsächlich passender Kandidat. Wenn unterschiedliche Voraussetzungen verschiedene Restfähigkeiten erlauben, getrennte Zweige statt einer irreführenden Vereinigung. Keine sinnlose Pflichtanzahl an Zweigen; keine branchweisen Vollständigkeitsbeweise behaupten. Auf eine endliche Alarmmeldung darf z.B. nicht unbemerkt Erhaltung sämtlicher offener Vorfälle übertragen werden.

### 3. `a7/report.md`
Einfache deutsche Zusammenfassung: neu erkannte Zustandsmechanismen, konkrete Residue-Splits/Wiederverwendungen, Modulgrenzen, Gegenbeispiele, ungelöste Voraussetzungen und Prüfbedarf. Erkläre besonders bisher offene Fälle. Keine endgültige globale Anzahl, kein empirischer Nachweis behaupten.

## Vor Abschluss selbst prüfen

- Jeder zugeteilte Stressor genau einmal, keine fremden IDs.
- Jeder Zweig begründet und korrekt typisiert, alle referenzierten Residues vorhanden und verwendet.
- Historische Quelldateien nicht verändert; angemessene Reviewauflagen übernommen.
- Keine hypothetische Schutzstruktur als bereits überlebendes Produktfeature ausgegeben.
- Materialentscheidungen und Ergebnis gegen den beauftragten Run dokumentiert.
