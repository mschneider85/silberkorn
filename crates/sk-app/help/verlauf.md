# Verlauf und Anpassungen

Jede Änderung lässt sich zurücknehmen, ein Stand lässt sich sichern, und Einstellungen lassen sich von einem Bild auf andere übertragen.

## Rückgängig und Wiederholen

**Rückgängig** ({taste:undo}) nimmt den letzten Schritt zurück, **Wiederholen** ({taste:redo}) stellt ihn wieder her. Das gilt nicht nur für Regler, sondern auch für Bewertung, Farb-Label und Keywords sowie für Änderungen an mehreren Bildern auf einmal (Einfügen, Stile, Automatisch anpassen, Look angleichen).

Ein Zug an einem Regler ist ein Schritt, ebenso mehrere Pfeiltasten- oder Mausrad-Schritte kurz hintereinander.

## Verlauf

Der Tab **Verlauf** listet alle Schritte des geöffneten Bildes, benannt nach dem, was sich geändert hat, etwa „Belichtung 0.8“. Ganz unten steht **Geöffnet**, der Stand beim Öffnen.

- Überfahren eines Schritts zeigt ihn im Bild, ohne ihn anzuwenden.
- Ein Klick springt zu diesem Stand. Spätere Schritte bleiben grau erreichbar, bis eine neue Änderung sie verwirft.

Der Verlauf gilt, solange das Bild geöffnet ist. Wer einen Stand dauerhaft behalten will, legt einen Schnappschuss an. Die letzten Schritte zeigt auch die Seitenleiste neben dem Bild.

## Schnappschüsse

Ein Schnappschuss sichert den jetzigen Stand unter einem Namen, dauerhaft für dieses Bild bzw. diese Variante. So lassen sich etwa eine warme und eine kühle Fassung festhalten und vergleichen.

- **+ Schnappschuss** im Tab **Verlauf** (oder in der Seitenleiste) legt einen an.
- Die Schnappschüsse erscheinen als kleine Vorschauen. Überfahren zeigt einen im Bild, ein Klick wendet ihn an (ein Rückgängig-Schritt), ein Doppelklick benennt ihn um.
- Rechtsklick bietet **Anwenden**, **Umbenennen** und **Löschen …**.

Mit **Liste** im Tab **Stile** erscheinen Schnappschüsse wie Stile als knappe Liste.

## Anpassungen kopieren

Silberkorn überträgt Werkzeuge, nicht immer den ganzen Satz an Einstellungen. Alles, was nicht übertragen wird, behalten die Zielbilder.

- **Anpassungen kopieren** ({taste:copy-adjustments}) merkt sich die bearbeiteten Werkzeuge des Bildes.
- **Anpassungen einfügen** ({taste:paste-adjustments}) wendet genau diese auf alle ausgewählten Bilder an.

Zuschnitt, Drehen & Spiegeln, Trapezkorrektur, Ebenen und Retusche gehören zum Bildinhalt und werden nur übertragen, wenn man sie ausdrücklich wählt.

### Anpassungen auswählen und kopieren

**Anpassungen auswählen und kopieren …** (Menü **Bearbeiten** oder Rechtsklick auf das Kopieren-Symbol der Werkzeugleiste) zeigt alle Werkzeuge zum Ankreuzen; bearbeitete sind hervorgehoben. **Bearbeitete**, **Alle** und **Keine** wählen schnell. **Kopieren** legt die Wahl zum Einfügen bereit, **Auf … anwenden** überträgt sie gleich auf die übrigen ausgewählten Bilder.

### Ein Werkzeug übertragen

Sind mehrere Bilder ausgewählt, erscheint im Kopf jedes Werkzeugs ein Kopieren-Symbol. Es überträgt nur dieses Werkzeug vom angezeigten Bild auf die anderen ausgewählten, etwa nur den Weißabgleich einer Serie.

## Zurücksetzen

- ↺ im Kopf eines Werkzeugs setzt nur dieses zurück. Mit gehaltener Umschalttaste zeigt der Knopf, solange er gedrückt ist, das Bild ohne dieses Werkzeug.
- Doppelklick auf den Namen eines Reglers setzt nur diesen Regler zurück.
- **Alle Anpassungen zurücksetzen** (Menü **Bearbeiten** oder ↺ in der Werkzeugleiste) stellt den Zustand eines neuen Bildes her. Mit gehaltener Umschalttaste zeigt der Knopf das Bild nur vorübergehend ohne Anpassungen.

Auch Zurücksetzen lässt sich mit {taste:undo} zurücknehmen.

## Automatisch einstellen

Werkzeuge, die sich selbst einstellen können, tragen im Kopf ein **A**. Es stellt nur dieses Werkzeug ein, alle anderen Regler bleiben, wie sie sind. Wie **Automatisch anpassen** ({taste:auto-adjust}) wirkt es auf alle ausgewählten Bilder, im Hintergrund.

Ein **A** haben unter anderem **Weißabgleich**, **Belichtung**, **Lichter & Tiefen**, **Farbe**, **Tonwerte**, **Drehen & Spiegeln**, **Trapezkorrektur**, **Objektivkorrektur**, **Schärfen**, **Rauschreduzierung** und **Verbessern**. Jedes A ist auch ein Befehl in der Befehlssuche (etwa **Weißabgleich automatisch**) und kann ein eigenes Tastenkürzel bekommen. Was die einzelnen Werkzeuge dabei einstellen, steht im Kapitel [Entwickeln](entwickeln).

> Auf einer Ebene sind A, Einfügen und Automatisch anpassen deaktiviert: Sie wirken nur auf das ganze Bild.
