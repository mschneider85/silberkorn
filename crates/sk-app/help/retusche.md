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
- Bei **Entfernen** zusätzlich **Fluss**, **Kanten beachten** und **Toleranz** wie beim Maskenpinsel, hinter dem Knopf **Mehr**; siehe [Pinsel einstellen](ebenen-masken#Pinsel einstellen).

Die Einstellungen stehen in der Leiste unten im Bild, zusammen mit den drei Werkzeugen und **Fertig** (Esc).

Die Stelle großzügig, aber ganz übermalen, samt Schatten und Spiegelungen des Objekts.

## Quelle festlegen

Reparieren und Klonen nehmen ihre Quelle automatisch. Mit aktivem Werkzeug zeigt das Bild alle Bereiche mit Pins: Der weiße verschiebt den Bereich, der gelbe (beim gewählten Bereich) die Quelle.

Eine Quelle von Hand festlegen wie beim Kopierstempel:

- **Alt-Klick** (ohne zu ziehen) ins Bild legt die Quelle fest: für den gewählten Bereich sofort, ohne gewählten Bereich für den nächsten Strich. Ein Fadenkreuz zeigt sie bis dahin.
- Den gelben Pin auf eine passende Stelle ziehen.
- **Neu wählen** bei **Quelle** in der gewählten Retusche sucht automatisch eine andere.

## Staub und Flecken finden

**Finden** in der Zeile **Staub und Flecken** sucht kleine, runde, dunkle Flecken auf ruhigen Flächen wie Himmel, Wänden oder Haut und legt für jeden eine Reparieren-Stelle an. Typisch für Sensorstaub bei kleiner Blende.

- **Empfindlichkeit** (die Zeile aufklappen): höher findet auch schwächere Flecken, aber eher auch falsche.
- Bereits reparierte Stellen werden übersprungen.
- Danach zeigt das Werkzeug Reparieren alle Stellen zum Prüfen; Falsche mit {taste:delete-spot} löschen.

Den Befehl gibt es auch in der Befehlssuche (Strg+K).

## Retuschen

Alle Retuschen des Bildes stehen als Karten wie die Ebenen, die entfernten zuerst: ein Ausschnitt der Stelle mit dem Symbol der Art, Name, Weichheit (bei Entfernen „KI-Füllung“) und Deckkraft. Ohne Retusche fehlt der Abschnitt.

- Klick auf eine Karte oder auf den Pin im Bild wählt einen Bereich. Die gewählte Karte zeigt seine Einstellungen.
- **Deckkraft**: Stärke des Bereichs.
- **Reparieren | Klonen** wechselt die Art, **Weichheit** ändert die Kante.
- **Quelle**: **Neu wählen** sucht die Quelle erneut automatisch.
- **Löschen** unten in der gewählten Karte oder {taste:delete-spot} löscht sie, ↺ im Kopf alle. Das Kontextmenü einer Karte hat Löschen auch.

Entfernte Bereiche haben weder Art noch Quelle.

### Bereich erweitern oder verkleinern

Jeder Strich ergibt eine neue Retusche. Um eine gewählte zu ändern, in ihrer Karte **Pinsel** (dazumalen) oder **Radierer** (wegnehmen) einschalten; ein zweiter Klick schaltet aus, ebenso die Wahl einer anderen Retusche. Ohne Umschalten geht es mit gedrückter Umschalttaste (dazumalen) oder Alt-Taste (wegnehmen). Ein entfernter Bereich wird danach mit allen seinen Strichen neu gefüllt.

> Bei Entfernen zeigt das Bild umrandet, was entfernt wurde. Der erste Gebrauch lädt das KI-Modell; mehr unter [KI-Werkzeuge](ki-werkzeuge#Entfernen).
