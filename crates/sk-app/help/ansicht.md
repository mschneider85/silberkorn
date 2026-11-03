# Ansicht

Wie das Bild gezeigt wird, ändert nie die Bearbeitung. Die meisten Ansichten stehen im Menü **Ansicht** und in der Werkzeugleiste; Esc beendet sie der Reihe nach.

## Raster und Einzelbild

Das **Raster** zeigt alle Bilder der Ansicht als Vorschaubilder mit Sternen, Farb-Label und Stapelzahl. Das **Einzelbild** zeigt ein Bild groß zum Bearbeiten. {taste:grid} oder die Knöpfe **Raster** und **Einzelbild** in der Werkzeugleiste schalten um, ein Doppelklick im Raster öffnet das Bild.

Strg+Mausrad im Raster (oder der Regler rechts in der Statusleiste) ändert die Größe der Vorschaubilder. {taste:previous} und {taste:next} blättern, im Raster gehen {taste:row-up} und {taste:row-down} eine Zeile weiter. Mit gehaltener Umschalttaste erweitern die Pfeiltasten die Auswahl.

### Filmstreifen

Unter dem Einzelbild zeigt der Filmstreifen die Bilder der Ansicht zum schnellen Wechseln. **Filmstreifen** im Menü **Ansicht** blendet ihn aus.

## Zoom

- {taste:zoom-100} oder Doppelklick ins Bild wechselt zwischen **Einpassen** und **100 %** (ein Bildpixel je Bildschirmpixel).
- Das Mausrad zoomt stufenlos, Ziehen verschiebt den Ausschnitt; mit gehaltener Leertaste verschiebt Ziehen auch, während ein Werkzeug aktiv ist.
- Das Feld **Zoom** im Bild zeigt die Stufe: ziehen oder klicken und eine Zahl eingeben.

Hineingezoomt wird der sichtbare Ausschnitt in voller Auflösung nachgerechnet.

### Navigator

Beim Zoomen zeigt ein kleines Gesamtbild, welcher Ausschnitt gerade zu sehen ist; Klicken oder Ziehen darin verschiebt ihn. **Navigator beim Zoomen** im Menü **Ansicht** schaltet ihn ab.

### Lupe

Die Maustaste auf dem eingepassten Bild gedrückt halten: Unter dem Zeiger erscheint ein Ausschnitt in 100 %. Loslassen beendet die Lupe. So prüft man Schärfe und Rauschen, ohne den Zoom zu verlassen.

## Vorher und Nachher

Drei Ansichten vergleichen das Bild mit seinem unbearbeiteten Zustand:

- **Vorher / Nachher** ({taste:before}) wechselt das ganze Bild auf „vorher“.
- **Geteilter Vergleich** ({taste:split}) teilt das Bild mit einer verschiebbaren Trennlinie: links vorher, rechts nachher.
- **Vorher | Nachher nebeneinander** ({taste:side-by-side}) zeigt beide Fassungen nebeneinander, mit gemeinsamem Zoom und Ausschnitt.

Mehrere verschiedene Bilder nebeneinander zeigt [Auswahl nebeneinander vergleichen](vergleichen#Auswahl nebeneinander vergleichen).

## Vollbild ohne Ablenkung

**Vollbild ohne Ablenkung** ({taste:fullscreen}) zeigt nur das Bild, auf schwarzem Grund. Esc beendet das Vollbild. Bewerten und Blättern funktionieren weiter, was sich zum Sichten eignet.

## Bild in eigenem Fenster

**Bild in eigenem Fenster** ({taste:viewer-window}) öffnet das Bild als eigenes Fenster, etwa für einen zweiten Bildschirm. Das Hauptfenster zeigt dann das Raster zum Sichten und Auswählen, die Werkzeuge bleiben daneben. Im Bildfenster funktionieren Zoom, Vergleichsansichten, Werkzeuge und alle Tastenkürzel; F11 schaltet dort auf Vollbild, Esc zurück. Position und Größe werden gemerkt.

## Messanzeige

Die Messanzeige steht in der Seitenleiste neben dem Bild, ist diese ausgeblendet, oben im Tab **Belichtung**. **Messanzeige** ({taste:scope}, Menü **Ansicht** oder Rechtsklick darauf) wechselt zwischen **Histogramm**, **Wellenform** und **Vektorskop**. Die Wahl bleibt über Programmstarts erhalten.

### Histogramm

Das Histogramm zeigt die Verteilung der Helligkeiten je Farbkanal, links Schwarz, rechts Weiß. Steht die Maus im Bild, markieren Linien den Farbwert unter dem Zeiger, daneben stehen die Werte in Prozent.

Darüber liegen die Tonbereiche **Schwarz**, **Tiefen**, **Belichtung**, **Lichter** und **Weiß**. Ziehen in einem Bereich verstellt den passenden Regler direkt. Umgekehrt markiert ein Regler im Tab **Belichtung** seinen Bereich, solange die Maus darauf steht.

### Wellenform

Die Wellenform zeigt für jede Spalte des Bildes, wie hell die Pixel dort sind (unten Schwarz, oben Weiß), Rot, Grün und Blau übereinander. Anders als im Histogramm bleibt jeder Tonwert an seiner Stelle: Ein heller Himmel links steht auch links oben.

### Vektorskop

Das Vektorskop zeigt die Farben nach Farbton (Winkel) und Sättigung (Abstand von der Mitte). Kästchen markieren Rot, Gelb, Grün, Cyan, Blau und Magenta. Die Linie zwischen Rot und Gelb ist die Hauttonlinie: Haut jeder Farbe liegt auf ihr, nur verschieden weit außen. Die Farbe unter dem Zeiger erscheint als weißer Punkt.

## Belichtungswarnung

**Belichtungswarnung** ({taste:clip-warning}) färbt ausgefressene Lichter rot und abgesoffene Tiefen blau. Auch die beiden kleinen Dreiecke in den oberen Ecken des Histogramms schalten sie um.

Gezielter geht es beim Regeln: Alt beim Ziehen eines Reglers wie **Belichtung**, **Lichter** oder **Schwarz** zeigt nur die Stellen, die dabei beschnitten werden.

## Schärfemaske

**Schärfemaske** ({taste:focus-mask}) markiert scharfe Bereiche grün, auch auf den Vorschaubildern und in der Vergleichsansicht. So findet man schnell das schärfste Bild einer Serie. Die **Empfindlichkeit** im Menü **Ansicht** bestimmt, ab wann ein Bereich als scharf gilt.

## Hilfslinien

**Hilfslinien wechseln** ({taste:guides-next}) blendet nacheinander **Drittel**, **Goldener Schnitt**, **Raster** und **Diagonalen** über das Bild und wieder **Keine**. Direkt wählen lassen sie sich im Menü **Ansicht → Hilfslinien** oder über das Raster-Symbol in der Werkzeugleiste.

## Bildinfo

**Bildinfo: kurz / ausführlich / aus** ({taste:info-overlay}) blendet oben links über dem Bild Dateiname und Belichtung ein (Zeit, Blende, ISO, Brennweite). Ein zweiter Druck zeigt dazu Kamera, Objektiv, Aufnahmedatum und Größe, ein dritter blendet aus. Alle Aufnahmedaten stehen im Tab **Metadaten** unter **Aufnahme**.

## Hintergrund

**Hintergrund** im Menü **Ansicht** (oder Rechtsklick neben das Bild) wählt **Dunkelgrau**, **Schwarz**, **Mittelgrau** oder **Weiß**. Die Umgebung beeinflusst, wie hell ein Bild wirkt; vor dem Drucken sind Mittelgrau oder Weiß ehrlicher.

## Seitenleiste

Rechts neben dem Bild zeigt die Seitenleiste Navigator, Messanzeige, Bildinfo, eine Übersicht der Bearbeitung (ein Klick springt zum Werkzeug), die Schnappschüsse und die letzten Schritte des Verlaufs. Im Menü **Ansicht** wählt man **Seitenleiste: wenn Platz ist** (Standard: nur, wenn das Bild dadurch nicht kleiner wird), **Seitenleiste: immer** oder **Seitenleiste: aus**.

## Softproof

**Softproof** ({taste:softproof}) zeigt das Bild so, wie es in einem anderen Farbraum oder auf einem Drucker herauskommt. Profil und Optionen stehen im Tab **Ausgabe**, siehe [Softproof](ausgabe#Softproof). Dort stehen auch die [Ausgabevorschau](ausgabe#Ausgabevorschau) und die [Druckvorschau](ausgabe#Druckvorschau).
