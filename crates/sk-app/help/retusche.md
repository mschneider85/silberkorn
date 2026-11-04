# Retusche

Der Tab **Retusche** entfernt Störendes: Sensorstaub, Hautunreinheiten, Stromleitungen, Passanten. Alle Retuschen sind nicht-destruktiv, bleiben beim Zuschneiden oder Drehen am Bildinhalt und lassen sich jederzeit ändern oder löschen. Retusche wirkt immer auf das ganze Bild, nicht auf Ebenen.

## Werkzeuge

Drei Werkzeuge, die jeweils über die Stelle gemalt werden; ein einfacher Klick ergibt einen runden Punkt.

- **Reparieren** ({taste:tool-heal}): übernimmt Struktur von einer automatisch gewählten Quelle und gleicht Farbe und Helligkeit an die Umgebung an. Für Flecken, Staub und kleine Störungen auf Haut, Himmel oder Wänden.
- **Klonen** ({taste:tool-clone}): kopiert eine Quelle 1:1. Wenn Struktur genau fortgesetzt werden muss, etwa an Kanten, oder wenn Reparieren die Farbe verfälscht.
- **Entfernen** ({taste:tool-remove}): die KI füllt die Stelle passend zur Umgebung. Für größere Objekte wie Personen, Schilder oder Leitungen.

### Pinsel einstellen

- **Größe**: Durchmesser. {taste:brush-smaller} und {taste:brush-larger} ändern sie schrittweise; Ziehen mit der rechten Maustaste ändert Größe (links und rechts) und Härte (hoch und runter) im Bild.
- **Härte**: weiche oder harte Kante.
- **Deckkraft**: wie stark die Retusche wirkt.
- Bei **Entfernen** zusätzlich **Fluss**, **Kanten beachten** und **Toleranz** wie beim Maskenpinsel, siehe [Pinsel einstellen](ebenen-masken#Pinsel einstellen).

Größe, Härte und Deckkraft stehen auch in der Leiste unten im Bild, zusammen mit den drei Werkzeugen, **Löschen** für die gewählte Retusche und **Fertig** (Esc).

Die Stelle großzügig, aber ganz übermalen, samt Schatten und Spiegelungen des Objekts.

## Quelle festlegen

Reparieren und Klonen nehmen ihre Quelle automatisch. Mit aktivem Werkzeug zeigt das Bild alle Bereiche mit Pins: Der weiße verschiebt den Bereich, der gelbe (beim gewählten Bereich) die Quelle.

Eine Quelle von Hand festlegen wie beim Kopierstempel:

- **Alt-Klick** (ohne zu ziehen) ins Bild legt die Quelle fest: für den gewählten Bereich sofort, ohne gewählten Bereich für den nächsten Strich. Ein Fadenkreuz zeigt sie bis dahin.
- Den gelben Pin auf eine passende Stelle ziehen.
- **Neue Quelle** in der Liste sucht automatisch eine andere.

## Staub und Flecken finden

**Staub und Flecken finden** sucht kleine, runde, dunkle Flecken auf ruhigen Flächen wie Himmel, Wänden oder Haut und legt für jeden eine Reparieren-Stelle an. Typisch für Sensorstaub bei kleiner Blende.

- **Empfindlichkeit**: höher findet auch schwächere Flecken, aber eher auch falsche.
- Bereits reparierte Stellen werden übersprungen.
- Danach zeigt das Werkzeug Reparieren alle Stellen zum Prüfen; Falsche mit {taste:delete-spot} löschen.

Den Befehl gibt es auch in der Befehlssuche (Strg+K).

## Retuschen

Alle Retuschen des Bildes stehen in einer Liste wie die Ebenen: Art, Name und Deckkraft, die entfernten zuerst.

- Klick in der Liste oder auf den Pin im Bild wählt einen Bereich.
- **Deckkraft** über der Liste ändert die Stärke des gewählten Bereichs.
- **Art** wechselt zwischen Reparieren und Klonen, **Weichheit** ändert die Kante.
- **Neue Quelle** wählt die Quelle erneut automatisch.
- **Löschen** oder {taste:delete-spot} löscht den gewählten Bereich, ↺ im Kopf alle.

Was eine Art nicht hat, ist ausgegraut, etwa die Quelle bei Entfernen.

### Bereich erweitern oder verkleinern

Einen gewählten Bereich ändert man, indem man mit gedrückter Umschalttaste dazumalt oder mit gedrückter Alt-Taste einen Teil wegnimmt. Dasselbe geht über **Malen** in der Liste: **Neu**, **Erweitern** oder **Verkleinern**. Ein entfernter Bereich wird danach mit allen seinen Strichen neu gefüllt.

> Bei Entfernen zeigt das Bild umrandet, was entfernt wurde. Der erste Gebrauch lädt das KI-Modell; mehr unter [KI-Werkzeuge](ki-werkzeuge#Entfernen).
