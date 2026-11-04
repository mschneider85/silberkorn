# Ebenen & Masken

Mit Ebenen wirken Anpassungen nur auf einen Teil des Bildes: den Himmel abdunkeln, ein Gesicht aufhellen, den Hintergrund entsättigen. Jede Ebene hat eine **Maske**, die festlegt, wo sie wirkt, und eigene **Anpassungen**, die festlegen, was sie ändert. Die Bearbeitung ohne Ebene heißt **Hintergrund**.

Der Tab **Ebenen** zeigt von oben nach unten: welche Ebene, wo sie wirkt, was sie ändert.

## Ebenen

Die Liste zeigt die oberste Ebene zuerst, ganz unten den Hintergrund.

- Klick wählt eine Ebene, Doppelklick benennt sie um.
- Das Auge blendet eine Ebene aus. Alt-Klick auf das Auge zeigt nur diese Ebene, ein zweiter wieder alle.
- **Deckkraft** über der Liste bestimmt, wie stark die gewählte Ebene wirkt.
- Das Papierkorb-Symbol löscht die gewählte Ebene ({taste:delete-spot}).

### Ebene anlegen

**+** unter der Liste bietet:

- **Neue Ebene**: mit leerer Maske; danach malen oder auswählen.
- **Neue Ebene mit Stilpinsel**: eine Ebene mit fertigen Anpassungen, der Pinsel ist gleich gewählt. Zur Wahl stehen Aufhellen, Abdunkeln, Wärmer, Kühler, Mehr Farbe, Entsättigen, Details betonen, Haut glätten, Lichter zurückholen, Tiefen aufhellen, Dunst entfernen und Augen aufhellen.
- **Neue Ebene mit Look**: eine Ebene über das ganze Bild mit einem Filmlook oder eigenen Stil. Ihre Deckkraft ist die Stärke, ihre Maske kann den Look auf einen Bereich beschränken.

Pinsel, Verläufe und Zauberpinsel legen selbst eine Ebene an, wenn keine gewählt ist. Motiv, Himmel und Tiefe legen eine Ebene mit passendem Namen an.

### Zwischen Ebenen wechseln

Oben in den Tabs Belichtung, Farbe und Details steht, woran die Werkzeuge gerade arbeiten: **Hintergrund** oder blau die gewählte Ebene. Über das Menü dort lässt sich eine andere Ebene wählen oder eine neue anlegen; beim Überfahren der Leiste wird die Maske eingeblendet.

- {taste:layer-up} und {taste:layer-down} wählen die Ebene darüber bzw. darunter; unter der untersten liegt der Hintergrund.
- ✕ in der Leiste kehrt zum Hintergrund zurück.

## Maske

Legt fest, wo die gewählte Ebene wirkt. Mehrere Werkzeuge lassen sich in einer Maske kombinieren.

### Werkzeuge

- **Pinsel** ({taste:tool-brush}): malt die Maske. Mit gehaltener Alt-Taste radiert er.
- **Radierer** ({taste:tool-eraser}): nimmt Teile der Maske weg.
- **Linear** ({taste:tool-linear}): ein Verlauf mit weichem Übergang, etwa für einen Himmel.
- **Radial** ({taste:tool-radial}): ein elliptischer Verlauf, etwa um ein Gesicht.
- **Zauberpinsel** ({taste:tool-magic}): über einen Bereich malen; ausgewählt wird, was zusammenhängt und den überstrichenen Farben ähnelt. **Größe** und **Toleranz** sind einstellbar.
- **Objekt** ({taste:tool-object}): auf ein Objekt klicken, es wird ausgewählt (KI). Siehe [Objekt auswählen](ebenen-masken#Objekt auswählen).

Darunter:

- **Füllen**: Maske über das ganze Bild, etwa um sie danach unter **Eingrenzen** zu beschränken.
- **Leeren**: Maske löschen.
- **Invertieren**: kehrt die Maske um.
- **Maske anzeigen** ({taste:show-mask}): färbt die Maske rot ein.

↺ im Kopf setzt die Maske zurück.

### Pinsel einstellen

Pinsel und Radierer haben eigene, gespeicherte Einstellungen:

- **Größe**: Durchmesser. {taste:brush-smaller} und {taste:brush-larger} ändern sie schrittweise.
- **Härte**: weiche oder harte Kante.
- **Deckkraft**: höchste Deckung eines Strichs.
- **Fluss**: wie viel jeder Abdruck beiträgt; innerhalb eines Strichs baut sich die Deckung auf.
- **Kanten beachten**: der Strich wirkt nur, wo das Bild der Stelle unter der Pinselmitte ähnelt; hält Kanten wie den Horizont sauber. **Toleranz** bestimmt, wie verschieden es sein darf.

Ziehen mit der rechten Maustaste ändert die Größe (nach links und rechts) und die Härte (nach oben und unten) direkt im Bild.

Solange ein Werkzeug der Maske gewählt ist, steht unten im Bild eine Leiste: die Werkzeuge nebeneinander (etwa zum Wechsel zwischen Pinsel und Radierer), **Größe**, **Härte** und **Deckkraft** des Pinsels (beim Zauberpinsel **Größe** und **Toleranz**), **Maske anzeigen**, bei einem gewählten Verlauf **Löschen** und **Fertig** (Esc), das das Werkzeug beendet. Was gemalt ist, bleibt.

### Verläufe ändern

Mit gewähltem Verlaufswerkzeug zeigt das Bild drei Linien bzw. Ellipsen: volle Wirkung, Mitte und Ende des Übergangs.

- **Linear**: Mittellinie oder Mittelpunkt verschieben den Verlauf, die äußeren Linien machen den Übergang breiter oder schmaler, der runde Griff dreht ihn.
- **Radial**: Der Mittelpunkt verschiebt, die mittlere Ellipse skaliert, ihre eckigen Griffe ändern Breite oder Höhe, innere und äußere Ellipse die weiche Kante, der runde Griff dreht.
- Ziehen an einer freien Stelle legt einen neuen Verlauf an; {taste:delete-spot} löscht den gewählten.

### Objekt auswählen

1. **Objekt** wählen. Das Bild wird einmal analysiert.
2. Auf das Objekt klicken.
3. Weitere Klicks verfeinern die Auswahl; Umschalt+Klick wählt ein weiteres Objekt dazu, Alt+Klick nimmt Teile aus.

Jeder Klick ist ein Rückgängig-Schritt.

### Automatisch auswählen

Die Knöpfe unter **Automatisch auswählen** erzeugen eine Maske aus dem Bildinhalt (KI, siehe [KI-Werkzeuge](ki-werkzeuge#Motiv, Himmel und Tiefe)):

- **Motiv**: das Hauptmotiv, auch mit Haaren und feinen Teilen.
- **Himmel**: den Himmel.
- **Tiefe**: nach Entfernung, anfangs den nahen Teil des Bildes. Den Bereich stellt danach ein Trapez über der Verteilung der Entfernungen ein, von nah (links) bis fern (rechts), mit getrenntem Übergang nach vorn und hinten.

**Auswahl entfernen** löscht diese automatische Auswahl; Pinselstriche und Verläufe bleiben. Mit Pinsel und Radierer lässt sich die Auswahl nacharbeiten.

### Eingrenzen

Beschränkt die ganze Maske auf einen Helligkeits- oder Farbbereich, etwa einen Verlauf nur auf den hellen Himmel und nicht auf die Bäume davor.

- **Helligkeitsbereich**: ein Trapez über dem Histogramm. Zwischen den oberen Griffen wirkt die Ebene voll, zu den unteren läuft sie aus. Das Trapez lässt sich auch als Ganzes verschieben.
- **Farbbereich**: mit **Farbe aufnehmen** auf eine Farbe im Bild klicken; **Farbton**, **Breite** und **Weichheit** legen den erfassten Bereich fest, **Min. Chroma** schließt fast neutrale Farben aus.

## Anpassungen

Was die gewählte Ebene ändert. Die Liste zeigt jede Änderung mit ihrem Wert; ein Klick springt zum Werkzeug, ↺ daneben setzt sie zurück, ↺ im Kopf alle.

Ist eine Ebene gewählt, wirken diese Regler in den Tabs Belichtung, Farbe und Details auf sie:

- **Belichtung**: Belichtung, Kontrast, Helligkeit.
- **Lichter & Tiefen**: Lichter, Tiefen, Weiß, Schwarz.
- **Gradationskurve**: alle Kanäle.
- **Weißabgleich**: Temperatur und Tönung, relativ zum Hintergrund.
- **Farbe**: Sättigung, Dynamik.
- **Farbeditor**: die acht Farbbereiche, ohne eigene Bereiche.
- **Farbbalance**.
- **Klarheit** mit Struktur, **Dunst entfernen**.

Die Knöpfe unter der Liste führen direkt zu den Tabs. Was nur aufs ganze Bild wirkt, ist im Ebenenmodus ausgegraut: etwa Tonwerte, Schärfen, Rauschreduzierung, die Tabs Objektiv, Effekte, Retusche und Stile sowie Automatisch anpassen.

> Eine neue Ebene ändert nichts, bis sie eine Maske hat. „Neue Ebene“ führt darum gleich zum Abschnitt Maske.

## Beispiel: Himmel abdunkeln

1. **Himmel** unter Automatisch auswählen klicken; eine Ebene „Himmel“ entsteht.
2. Im Tab Belichtung **Belichtung** senken, im Tab Farbe die **Dynamik** erhöhen.
3. Mit **Maske anzeigen** prüfen und wenn nötig mit dem Radierer nacharbeiten.
4. Mit **Deckkraft** die Wirkung insgesamt abschwächen.
