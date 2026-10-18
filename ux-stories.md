# UX-Stories (Stand 18.10.2026)

Die offenen Punkte aus `ux-audit.md`, zusammengefasst zu Stories aus Sicht der Nutzer.
Je Story: kurz, was heute passiert, was sich ändern würde, wie aufwendig das ist.
Die Nummern unter „Audit“ verweisen auf die Details (Fundstellen, Code) in `ux-audit.md`.

Zum Entscheiden ein `x` ins Kästchen setzen; bei Fragen oder Ergänzungen die Zeile „Input“ nutzen.

**Aufwand:** klein = unter einer Stunde · mittel = einige Stunden · groß = ein Tag oder mehr

---

## Bearbeiten und Rückgängig

### Story 1: Mehrere Bilder auf einmal bearbeiten – und es wieder zurücknehmen
**Heute:** Wer Anpassungen auf eine Auswahl einfügt, einen Stil anwendet oder „Automatisch anpassen“ bzw. „Look angleichen“ für mehrere Bilder startet, ändert alle diese Bilder sofort. Strg+Z nimmt nur das gerade offene Bild zurück, die anderen bleiben geändert. Es gibt auch keine Rückfrage.
**Ziel:** Strg+Z nimmt die ganze Mehrfach-Änderung in einem Schritt zurück.
**Aufwand:** groß · **Audit:** 6
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 2: Bewertung und Farb-Label rückgängig machen
**Heute:** Sterne, Farb-Labels und Keywords laufen am Verlauf vorbei. Wer nach einer Bewertung Strg+Z drückt, nimmt stattdessen die letzte Bildbearbeitung zurück – das überrascht.
**Ziel:** Auch Bewertung, Label und Keywords lassen sich mit Strg+Z zurücknehmen.
**Aufwand:** mittel · **Audit:** 7
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 3: Ein Zuschnitt = ein Rückgängig-Schritt
**Heute:** Beim Zuschneiden wird jeder einzelne Zug am Rahmen ein eigener Schritt im Verlauf. Wer den Rahmen fünfmal nachjustiert, muss fünfmal Strg+Z drücken.
**Ziel:** Der ganze Zuschnitt (vom Starten bis „Übernehmen“) ist ein Schritt.
**Aufwand:** klein · **Audit:** 14
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

---

## Werkzeuge im Bild

### Story 4: Werkzeuge im Bild verhalten sich gleich
**Heute:** Jedes Werkzeug, das im Bild arbeitet, ist etwas anders:
- Trapez-Linien haben „Übernehmen/Abbrechen“ im Panel *und* im Bild, der Zuschnitt nur im Panel.
- Geraderichten wirkt sofort beim Loslassen, ohne Übernehmen.
- Nur Zuschnitt und Trapez zeigen im Bild „Enter übernimmt, Esc bricht ab“; Geraderichten, Pipetten, Pinsel, Verlauf usw. zeigen nichts.
- Beim Wechsel zu einem anderen Bild enden Zuschnitt und Trapez (gesetzte Linien gehen still verloren), Pinsel und Verlauf bleiben aktiv.
- Beim Wechsel des Tabs bleibt der Zuschnitt im Bild aktiv, obwohl sein Panel weg ist.

**Ziel:** Ein Muster für alle: gleicher Hinweis im Bild, gleiche Knöpfe, und beim Bild- oder Tab-Wechsel enden alle Werkzeuge gleich (Linien und Zuschnitt werden dabei übernommen, nicht verworfen).
**Aufwand:** mittel · **Audit:** 15, 16, 17, 19
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 5: Ein Werkzeug öffnet seinen Tab
**Heute:** „W“ (Weißabgleich-Pipette) springt in den Farb-Tab. „Geraderichten“ springt aus der Befehlssuche in den Objektiv-Tab, aus der Werkzeugleiste aber nicht. „Zuschneiden“ (C) wechselt nie den Tab.
**Ziel:** Jedes Werkzeug öffnet beim Start immer den Tab mit seinen Einstellungen.
**Aufwand:** klein · **Audit:** 18
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 6: Esc verlässt auch Ansichten
**Heute:** Esc beendet Werkzeuge und das Vollbild. Vergleichsansicht, Vorher/Nachher, Druckvorschau, Softproof und 100 %-Zoom bleiben dagegen bei Esc bestehen.
**Ziel:** Esc kehrt auch aus diesen Ansichten zur normalen Ansicht zurück.
**Frage an dich:** Für welche Ansichten soll das gelten? (Vorschlag: Vergleich, Vorher/Nachher, 100 %-Zoom; Druckvorschau und Softproof eher nicht, weil man sie bewusst länger anlässt.)
**Aufwand:** klein · **Audit:** 20
**Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 7: Entf löscht, was gerade gewählt ist
**Heute:** Entf löscht nur einen gewählten Retuschepunkt oder Verlauf. Eine gewählte Ebene oder ein Bild im Raster lässt sich so nicht löschen (im Raster braucht es Strg+Entf).
**Ziel:** Entf löscht das, was gerade gewählt ist.
**Frage an dich:** Soll Entf im Raster Bilder in den Papierkorb legen, oder bleibt das bewusst bei Strg+Entf (Schutz vor Versehen)?
**Aufwand:** mittel · **Audit:** 31
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

---

## Regler und Bedienelemente

### Story 8: Nichts verschwindet – wirkungslose Regler werden grau
**Heute:** Eure Regel „deaktivieren statt ausblenden“ ist an einigen Stellen nicht umgesetzt. Ausgeblendet werden zum Beispiel die Masken-Knöpfe bei gewähltem Hintergrund, „Qualität“ bei Formaten ohne Qualität, „Hochformat“ beim Seitenverhältnis „Frei“ und die Einstellungen der Retusche, solange kein Punkt gewählt ist. Umgekehrt sind manche Regler aktiv, obwohl sie nichts bewirken (KI-Entrauschen bei HDR-DNG, Trapez-„Stärke“ bei 0/0). Abschnitte „nur ganzes Bild“ sehen im Ebenenmodus zweierlei aus.
**Ziel:** Überall gleich: sichtbar, aber grau, wenn ohne Wirkung.
**Aufwand:** mittel · **Audit:** 21, 22, 23, 24, 25, 26, 27
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 9: Zurücksetzen funktioniert überall gleich
**Heute:** Der ↺-Knopf im Abschnittskopf setzt nicht immer alles zurück: „Drehen & Spiegeln“ lässt die 90°-Drehung stehen, „Zuschnitt“ das Seitenverhältnis. Einige Abschnitte haben gar kein ↺ (Basiskurve, Look angleichen, Maske, Retusche, Rezept, Wasserzeichen), dafür Knöpfe mit drei verschiedenen Namen („Leeren“, „Alle löschen“, „Alle“).
**Ziel:** Jeder Abschnitt hat ↺ im Kopf, und es setzt den ganzen Abschnitt zurück; Inline-Knöpfe heißen einheitlich.
**Aufwand:** klein · **Audit:** 37, 38
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 10: Zahlen an Reglern einheitlich anzeigen
**Heute:** Regler zeigen „0.50“ mit Punkt, Statustexte „0,50“ mit Komma. Regler mit Minus-Bereich zeigen „12“ statt „+12“. Einheiten fehlen teils: Belichtung ohne „EV“, Geraderichten und Farbton ohne „°“, „Stärke“ mal mit, mal ohne „%“. Die Schnellbearbeitung zeigt „+5600.00“ für Kelvin.
**Ziel:** Überall Komma, „+“ bei positiven Werten, passende Einheit.
**Aufwand:** klein · **Audit:** 73, 74, 77, 78, 79, 81
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:** Änderung: statt überall Komma nehmen wir die amerikanische Schreibweise mit Punkt

### Story 11: Gleiche Größen auf gleicher Skala
**Heute:** Deckkraft ist beim Pinsel 0,05–1,00, bei Ebene, Retusche und Wasserzeichen 0–100 %. Ähnlich bei Härte, Fluss, Toleranz und Weichheit (mal 0–0,5, mal 0–60, mal 0–100 %). Tönung geht im Weißabgleich bis ±150, auf Ebenen bis ±100.
**Ziel:** Alles, was ein Anteil ist, wird in % angezeigt. Nur die Anzeige ändert sich, gespeicherte Bearbeitungen bleiben gültig.
**Frage an dich:** Soll Tönung angeglichen werden, oder ist der Unterschied zwischen ganzem Bild und Ebene gewollt?
**Aufwand:** mittel · **Audit:** 75, 76, 80
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:** Tönung angleichen

### Story 12: Schärfemasken-Regler wie alle anderen
**Heute:** Der Regler „Empfindlichkeit“ der Schärfemaske (Menü Ansicht) ist ein Standard-Regler: anderes Aussehen, kein Doppelklick zum Zurücksetzen, anderes Verhalten bei Mausrad und Pfeiltasten.
**Ziel:** Derselbe Regler wie überall sonst.
**Aufwand:** klein · **Audit:** 82
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

---

## Rückmeldung, Fehler, Löschen

### Story 13: Lange Aufgaben lassen sich überall abbrechen
**Heute:** Export, HDR, Entrauschen usw. haben ein ✕ in der Statusleiste. Entfernen, KI-Maske, Look angleichen, Automatisch anpassen, Indexieren und die Fleckensuche lassen sich nicht abbrechen; die Fortschrittsbalken in den Panels haben kein Abbrechen. Die Fleckensuche erscheint auch nicht in der Statusleiste.
**Ziel:** Wo ein Fortschrittsbalken ist, gibt es auch Abbrechen; alle Aufgaben erscheinen in der Statusleiste.
**Aufwand:** mittel · **Audit:** 32, 35
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 14: Fehler gehen nicht unter
**Heute:** Fehler landen an vier Orten: meist grau in der Statuszeile (und von der nächsten Meldung überschrieben), bei Entrauschen und Tiefe rot im Panel, beim Export nur im Protokoll im Ausgabe-Tab („Export abgeschlossen“ ohne Fehlerzahl), manche gar nicht (Stil oder Album löschen schlägt still fehl).
**Ziel:** Fehler erscheinen einheitlich gut sichtbar (rot, länger stehend); der Export meldet „abgeschlossen, 2 Fehler“.
**Aufwand:** mittel · **Audit:** 34
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 15: Vor dem endgültigen Löschen wird gefragt
**Heute:** Variante löschen und Papierkorb leeren fragen nach. Eigenen Stil löschen (die Datei ist weg), Album löschen und Schnappschuss löschen passieren sofort, ohne Rückfrage und ohne Rückgängig.
**Ziel:** Regel: Was sich nicht rückgängig machen lässt, wird vorher bestätigt.
**Aufwand:** klein · **Audit:** 36
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

---

## Dialoge, Listen, Menüs

### Story 16: Dialoge mit der Tastatur bedienen
**Heute:** Enter bestätigt nur in „Umbenennen“ und „Neue Session“; in allen anderen Dialogen braucht es die Maus. Der Beenden-Dialog nennt seinen Knopf „Abbrechen und beenden“ (missverständlich, weil „Abbrechen“ sonst „Dialog schließen“ heißt). „Personen zusammenführen“ lässt sich nicht rückgängig machen, der Knopf ist aber nicht rot.
**Ziel:** Enter bestätigt, Esc bricht ab – in allen Dialogen. Endgültige Aktionen sind rot, Knopftexte eindeutig.
**Aufwand:** klein · **Audit:** 41, 43, 44, 45
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 17: Umbenennen funktioniert überall gleich
**Heute:** Bei Ebenen stellt Esc den alten Namen wieder her, bei Schnappschüssen und Alben speichert Esc den neuen. Ebenen benennt man per Doppelklick um, Schnappschüsse auch, Alben nur über das Rechtsklick-Menü.
**Ziel:** Überall: Doppelklick startet, Enter speichert, Esc verwirft.
**Aufwand:** klein · **Audit:** 42
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 18: Schnappschüsse in der Seitenleiste wie im Verlauf-Tab
**Heute:** In der Seitenleiste fehlt das Umbenennen, der „+ Schnappschuss“-Knopf steht unten statt oben, es gibt keinen Hinweis bei leerer Liste. Der Tooltip „unter einem Namen sichern“ verspricht eine Namensabfrage, die es nicht gibt.
**Ziel:** Seitenleiste und Verlauf-Tab verhalten sich gleich.
**Aufwand:** klein · **Audit:** 40
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 19: Klarer Hinweis, wenn kein Bild gewählt ist
**Heute:** Die meisten Tabs zeigen ohne Bild „Kein Bild ausgewählt“. Der Stile-Tab zeigt dann nur halb gefüllte Bedienelemente ohne Erklärung. Der Info-Tab zeigt „Kein Bild ausgewählt“, obwohl im Raster Bilder ausgewählt sind und Bewertungen auf diese wirken.
**Ziel:** Überall derselbe Hinweis; der Info-Tab bearbeitet die Rasterauswahl.
**Aufwand:** klein · **Audit:** 39
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 20: Rechtsklick-Menüs wo man sie erwartet
**Heute:** Vorschaubilder, Alben, Stile und Personen haben ein Rechtsklick-Menü, Ebenen, Schnappschüsse, Look-Referenzen und Retuschepunkte nicht. Das Menü der Vorschaubilder bietet weder Bewertung, Label, Exportieren noch Anpassungen kopieren/einfügen.
**Ziel:** Rechtsklick-Menüs in allen Listen; das Vorschaubild-Menü bekommt die wichtigsten Foto-Befehle.
**Aufwand:** mittel · **Audit:** 47
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 21: Tastenkürzel in Tooltips stimmen immer
**Heute:** Einige Tooltips nennen Tasten fest im Text („Enter“, „Esc“, „Vorschau mit P“, „Strg+Umschalt+V“). Wer im Tastenkürzel-Editor umbelegt, sieht dort weiter die alte Taste. Der größte Teil ist schon umgestellt.
**Ziel:** Alle Tooltips zeigen die aktuell belegte Taste.
**Aufwand:** klein · **Audit:** 48
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

---

## Begriffe und Texte

### Story 22: Jede Funktion hat genau einen Namen
**Heute:** Dieselbe Funktion heißt an verschiedenen Stellen (Menü, Befehlssuche, Werkzeugleiste, Panel, README) unterschiedlich. Beispiele:
- Befehlssuche: fünf verschiedene Namen und Platzhalter
- „Anpassungen einfügen“ heißt auch „Anpassungen anwenden“ / „… auf Auswahl anwenden“
- Weißabgleich-Pipette heißt auch „Weißabgleich aufnehmen“ und „Weißabgleich“
- „Auto“ und „Automatisch“ nebeneinander; „Automatisch geraderichten“ meint die Perspektive, „Auto“ unter Drehen nur den Horizont
- „Tiefen“ und „Schatten“ für denselben Regler, „Mitten“ und „Mitteltöne“
- „Rauschen“, „Rauschreduzierung“, „KI-Entrauschen“
- „SPEED EDIT“ neben „Schnellbearbeitung“, „CA Rot/Cyan“ neben „Farbsaum Lila/Grün“, „Preset“ (README) neben „Stil“
- senkrecht/waagerecht neben vertikal/horizontal in einem Abschnitt
- Import, Referenzbild, Vorher/Nachher, Bildfenster: Menü und Befehlssuche formulieren verschieden

**Ziel:** Je Funktion ein Name überall. Englische Begriffe dürfen bleiben, wenn sie überall gleich sind. Ich schlage die Namen vor (stehen in `ux-audit.md`), du kannst einzelne ändern.
**Aufwand:** mittel · **Audit:** 49–55, 57, 59–63
**Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 23: Ein Name, der mehrere Dinge meint
**Heute:** Drei Begriffe stehen für verschiedene Dinge:
- **„Auswahl“** = die markierten Bilder *und* der Session-Ordner „Selects“ („Nach ‚Auswahl‘ verschieben“).
- **„Werkzeug“** = die Tabs, die Abschnitte im Panel *und* Pinsel/Verlauf usw.
- **„Entfernen“** = das KI-Retusche-Werkzeug *und* der allgemeine Knopf (Keyword, Rezept, Objektivprofil entfernen); bei Ebenen heißt es dagegen „Löschen“.

**Frage an dich:** Neue Namen für (a) den Ordner „Auswahl“, (b) die Tabs, (c) das Werkzeug „Entfernen“? Vorschläge: (a) „Ausgewählt“ oder „Selects“, (b) „Bereich“, (c) „Objekt entfernen“.
**Aufwand:** klein (sobald die Namen feststehen) · **Audit:** 58, 64, 65
**Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### Story 24: Meldungen und Schreibweise einheitlich
**Heute:** Fortschrittsmeldungen haben drei Satzformen („Berechne Maske…“, „Passe an …“, „Maske wird berechnet“), Zähler drei Formate („3 / 10“, „(3 von 10)“). Dazu Kleinigkeiten: „Bild(er)“ statt richtiger Mehrzahl, Großschreibung nach Doppelpunkt schwankt, eine einzige Du-Anrede (Kalibrieren), Tooltips mal mit, mal ohne Schlusspunkt, „&“ und „und“ gemischt.
**Ziel:** Eine Form je Fall, z. B. „Maske wird berechnet …“ und „Exportieren: 3 von 10“.
**Aufwand:** klein · **Audit:** 66–72
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:** Nimm eher die Form "Berechne Maske", "Passe an" in Fortschrittsmeldungen (möglichst kur eben)

---

## Optik

### Story 25: Symbole, Grautöne und Farben vereinheitlichen
**Heute:** „90° links drehen“ hat dasselbe Symbol wie „Zurücksetzen“. Löschen erscheint mal als Minus, mal als Mülleimer, mal als X. Hinweistexte („Noch keine Alben.“) haben fünf verschiedene Grautöne. Einige Farben (Warnrot, Hilfslinien-Gelb) sind fest im Code statt im Farbschema.
**Ziel:** Einheitliche Symbole, ein Grauton für Hinweise, alle Farben aus dem Farbschema. Für Nutzer ist das wenig sichtbar, es wirkt aber aufgeräumter.
**Aufwand:** klein bis mittel · **Audit:** 85–89
**Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**
