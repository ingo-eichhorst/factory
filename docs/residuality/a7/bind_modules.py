#!/usr/bin/env python3
"""Explicit coordinator assignments, NOT similarity clustering or residue merging."""
import argparse
import csv
import io
from pathlib import Path
import build

# Numeric suffixes are author IDs, not a target number of residues.
GROUPS = {
 'M01': {'OPR':'4 28', 'GVR':'1 20', 'OBR':'1 29 30', 'BDR':'13'},
 'M02': {'OPR':'22 25 26', 'GVR':'10 11 32', 'OBR':'7 27 34 35', 'BDR':'15'},
 'M03': {'OPR':'1 10 18 30', 'GVR':'13 17 18 19 28', 'OBR':'16 22 23 24 26 33', 'BDR':'24 25'},
 'M04': {'OPR':'11 12', 'OBR':'12 32', 'BDR':'28 29'},
 'M05': {'OPR':'9 14 15 17 29 31', 'GVR':'27', 'OBR':'19 20 21 28', 'BDR':'11'},
 'M06': {'OPR':'2 20 23', 'GVR':'5 30 34', 'OBR':'17', 'BDR':'9 35', 'KOR':'1'},
 'M07': {'OPR':'5 6 16', 'GVR':'3 15 22 24 29 36 37'},
 'M08': {'OPR':'34', 'GVR':'2 6 23 25 31', 'OBR':'11 14 15 25', 'BDR':'3 4 7 16 31 32'},
 'M09': {'OPR':'3 7 8', 'GVR':'12 16 26', 'OBR':'31 37', 'BDR':'14 22 33 34'},
 'M10': {'OPR':'19', 'GVR':'9', 'BDR':'5 12'},
 'M11': {'GVR':'4 7', 'OBR':'5 9 10 13', 'BDR':'2 30'},
 'M12': {'GVR':'33', 'OBR':'2 4 38 40', 'BDR':'1 21'},
 'M13': {'OPR':'32', 'OBR':'3 6 8', 'BDR':'17 18 19 37 39'},
 'M14': {'OPR':'33', 'GVR':'8', 'OBR':'36 39 41', 'BDR':'20'},
 'M15': {'GVR':'14', 'OBR':'42', 'BDR':'36'},
 'M16': {'OPR':'21 24 27', 'GVR':'21 35 38', 'OBR':'18', 'BDR':'6 8 10 23 26 27 38'},
}
CONTRACTS = {
 'M01': ('M07', 'Historische Fakten und Interpretation sind der Gegenstand. Aktuelle Wirklichkeit und fachliche Wahrheit werden nicht vom Speicher erzeugt.'),
 'M02': ('M01 M14', 'Der Gegenstand ist gebundene Identität oder Entscheidungsbefugnis. Geprüfte Rechte sind von tatsächlicher Teilnehmerannahme getrennt.'),
 'M03': ('M01 M02 M04', 'Erhalten wird ein abgegrenzter Arbeits- oder Verpflichtungsbestand. Grenzen müssen vor der jeweiligen Aufnahme wirken und gelten nicht für unkontrollierte Produzenten.'),
 'M04': ('M01 M02', 'Zeitbedeutung und zeitliche Disposition werden zentral geteilt. Daraus entsteht weder neue Vollmacht noch eine nachträgliche Wirkungskorrektur.'),
 'M05': ('M01 M02 M03 M10', 'Koordiniert werden konkrete Ausführung und bedienbarer Zugang. Tatsächliche Ressourcenausschlüsse brauchen zusätzliche belegte Grenzen.'),
 'M06': ('M01 M02 M04 M05 M16', 'Der eigene Operationsstand und seine offene Bedeutung gehören zur Koordination. Die entfernte Annahmegarantie verbleibt beim Teilnehmer.'),
 'M07': ('M01 M02 M15', 'Identische nutzbare Inhalte und ihre Herkunft bilden den Vertrag. Die fachliche Richtigkeit oder Rechtmäßigkeit folgt nicht allein aus Byteidentität.'),
 'M08': ('M01 M07 M11', 'Eine begrenzte überprüfbare Aussage oder Annahme ist der Gegenstand. Gegenbeleg und Tragweite bleiben ausdrücklich unabhängig von bloßem Erfolgstext.'),
 'M09': ('M01 M07 M10 M14 M15', 'Nutzbarkeit eines konkreten Wiederanlauf- oder Übergabestands verlangt die genannten Inhalte Leser Zugänge und Rechte gemeinsam.'),
 'M10': ('M07 M14', 'Der konkrete Integrationsvertrag und tatsächliche Inhalt werden gebunden. Bereits angenommene alte Wirkungen werden nicht durch ein Update beseitigt.'),
 'M11': ('M01 M03 M07 M15', 'Erhalten werden abgegrenzte beobachtete Daten oder sichere Darstellung. Eine Messung ersetzt keine Autorität Wahrheit oder unbeschränkte Ressourcen.'),
 'M12': ('M10 M14 M15', 'Die entscheidende Eigenschaft ist ein begrenzter Beleg außerhalb der genannten Fehler- oder Angreifergrenze. Keine zusätzliche Geschäftsautorität entsteht.'),
 'M13': ('M02 M03 M04 M11 M12', 'Gegenstand ist der konkrete menschlich nutzbare Fall oder Empfang. Darstellung Quittung Zuständigkeit und Reparatur bleiben getrennte Prädikate.'),
 'M14': ('M02 M10', 'Tatsächlicher Zugriff beziehungsweise legitimer Wiederzugang muss an einer tragfähigen Schutzgrenze bestehen. Der Scope-Name allein liefert sie nicht.'),
 'M15': ('M01 M02 M07 M13', 'Nutzung und Nachweis sind nur innerhalb einer legitim entschiedenen Datenverfügung brauchbar. Ein Register löst keinen unvereinbaren Rechtskonflikt.'),
 'M16': ('M02 M06 M10 M14', 'Der beanspruchte Fach- oder Ausschlussvertrag wird am tatsächlichen Wirkungspunkt erfüllt. Eine lokale Aufzeichnung oder ein Mock kann ihn nicht ersetzen.'),
}
OVERRIDES = {
 'KOR001': ('M01 M02 M03', 'Verbleibende Zustellautorisierung ist ein anderer Gegenstand als vergangene Versuchsevidenz. Neue Revision und alter Verbrauch werden atomar erhalten ohne daraus Geschäftsfreigabe oder Empfang abzuleiten.'),
 'GVR011': ('M01 M14 M16', 'M02 prüft eine frische Antwort der unabhängig verwalteten Widerrufsstelle über den M16-Autoritätsadapter. M01 bleibt historischer Präfix. Bei fehlender Gegenquelle bleibt nur ein engerer Halt und kein behaupteter aktueller Widerrufsstand.'),
 'BDR037': ('M12 M14 M16', 'Der unabhängig legitimierte Recoverybetreiber verantwortet die zusammengesetzte Fähigkeit. M12 liefert nur Vergleichsevidenz. Getrennte externe OS- oder Teilnehmerautorität entzieht Rechte außerhalb der Updategewalt; ein kompromittierter M02-Kern ist keine Voraussetzung dieses Entzugs.'),
 'GVR035': ('none', 'Autonomer qualifizierter lokaler Controller: kein laufendes Factory-Modul ist Voraussetzung seiner Sicherheitsreaktion. M02/M06/M14 betreffen nur vorgelagerte Factory-Aufträge oder spätere Belegübernahme.'),
 'BDR026': ('none', 'Der lokale Interlock prüft unabhängig mit eigener Energie Sensorik und Aktuatorgewalt. Factory-Befugnisse gelten nur für vorgelagerte Aufträge und sind kein Laufzeitbedarf des Sicherheitsentscheids.'),
 'BDR027': ('none', 'Der qualifizierte lokale Notstopp erreicht den anlagenspezifisch sicheren Zustand ohne Factory Cloud oder deren Authentisierung. Ein später importierter Beleg ist von der autonomen Wirkung getrennt.'),
 'OBR020': ('M01 M02 M03 M10 M14 M16', 'Ausführungskoordination sammelt Besitz- und Nachkommenevidenz. Tatsächlicher Entzug muss je OS-Ressource oder Teilnehmer durchgesetzt sein, sonst keine Wiedervergabe.'),
 'GVR021': ('M02 M06 M10 M14', 'Operations-ID und Ergebnisbeleg bleiben von der separat hergeleiteten Schreibepoche GVR038 getrennt. Eine historische Belegkopie beweist keine heute wirksame Deduplizierung.'),
 'GVR038': ('M02 M06 M10 M14', 'Eine wirksame Schreibepoche schützt auch gegen zwei verschiedene neue Operations-IDs. Sie ist deshalb nicht durch das Annahmebuch GVR021 oder eine lokale Lease ersetzt.'),
 'OPR027': ('M02 M06 M09 M10 M14', 'Clone-Inspektion und exklusive Sendebefugnis sind verschiedene Rollen. Exklusivität beruht auf tatsächlichem Rechteentzug oder Teilnehmerprüfung außerhalb kopierter Stores.'),
 'OBR029': ('M02 M03 M05', 'Cursor und rekonstruierbarer Ereignisbereich gehören zum Faktenkern. Transportpuffer und Gesamtaufnahme bleiben separat begrenzt und ein alter Cursor kann eine Lücke anzeigen.'),
 'GVR034': ('M01 M02 M03 M05', 'Die unmittelbare Dispatchsperre darf ohne freien Datenträger greifen. Ihre dauerhafte Bestätigung und Neustartbarriere bleiben gesondert nachzuweisen.'),
 'BDR039': ('M03 M08 M11 M12', 'Messung und Budget liefern begrenzte Tatsachen. Den Nutzen der Beobachtung gegenüber legitimen Dienstpflichten bewertet ein zuständiger Auftraggeber, kein universeller Kernel-Nutzenalgorithmus.'),
}


def render():
    residues = {r['id']:r for role in build.ROLES for r in build.read_csv(build.PROJECT/'reviews'/role/'a7/residues.csv')}
    for r in build.read_csv(build.ROOT/'additional-residues.csv'):
        build.require(r['id'] not in residues, 'Duplicate added residue')
        residues[r['id']] = r
    assignments = {}
    for mid,prefixes in GROUPS.items():
        for prefix,numbers in prefixes.items():
            for number in numbers.split():
                rid = f'{prefix}{int(number):03}'
                build.require(rid not in assignments,'Duplicate explicit assignment: '+rid)
                assignments[rid] = mid
    build.require(set(assignments)==set(residues), 'Review changed: missing '+str(set(residues)-set(assignments))+'; extra '+str(set(assignments)-set(residues)))
    rows=[]
    for rid,mid in sorted(assignments.items()):
        support,reason = OVERRIDES.get(rid,CONTRACTS[mid])
        rows.append(dict(residue_id=rid,primary_module=mid,support_modules=support,reason=reason))
    return build.csv_text(rows,['residue_id','primary_module','support_modules','reason'])


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check',action='store_true')
    args=parser.parse_args()
    text=render();path=build.ROOT/'module-bindings.csv'
    if args.check:
        build.require(path.exists() and path.read_text()==text,'Stale explicit module assignments')
    else:
        build.require(not path.exists(),'Use targeted edits or verify before replacing an existing assignment table')
        path.write_text(text,encoding='utf-8')
    print('Explicit module assignments checked' if args.check else 'Explicit module assignments written')


if __name__=='__main__':
    main()
