# KI-Werkzeuge

Einige Werkzeuge rechnen mit neuronalen Netzen. Sie tragen in der Oberfläche ein gelbes Funkeln hinter ihrem Namen: in den Abschnittsköpfen, an Knöpfen, in den Menüs und in der Befehlssuche. Alles läuft auf dem eigenen Rechner; kein Bild verlässt ihn.

## Modelle und Grafikkarte

Die meisten Netze werden nicht mit Silberkorn ausgeliefert, sondern beim Programmstart im Hintergrund heruntergeladen, einmalig und ohne Nachfrage. Der Startbildschirm und danach die Statusleiste zeigen den Fortschritt; man kann sofort arbeiten.

- Wird ein Werkzeug benutzt, bevor sein Modell da ist, zeigt das Werkzeug den Fortschritt und rechnet danach von selbst weiter.
- Ein fehlgeschlagener Download wird beim nächsten Gebrauch wiederholt.
- Was länger dauert, zeigt die Statusleiste mit Fortschritt; manches lässt sich dort mit ✕ abbrechen.
- Die Namen und Lizenzen der Modelle stehen unter **Hilfe → Lizenzen …**.

### Grafikkarte

**Einstellungen …**, Seite **Grafikkarte**, zeigt, wo Silberkorn rechnet:

- **Bildberechnung**: Regler, Vorschau und Export rechnen auf der Grafikkarte, mit dem Prozessor als Rückfall.
- **KI-Berechnung** (Windows): Die KI-Werkzeuge rechnen auf der Grafikkarte (jede Karte mit DirectX 12) ein Vielfaches schneller als auf dem Prozessor. Dafür lädt **Laden** einmalig eine Erweiterung von Microsoft.

Mit **Grafikkarte verwenden** lässt sich beides abschalten, etwa bei Problemen mit dem Grafiktreiber. Fällt die Grafikkarte aus, rechnet Silberkorn bis zum nächsten Start auf dem Prozessor weiter; der Dialog nennt den Grund.

## Verbessern

Abschnitt ganz oben im Tab **Details**, auch über **Entwickeln → Verbessern**. Zwei Netze verbessern die RAW-Daten selbst. Sie werden je Bild einmal im Hintergrund berechnet, das angezeigte Bild zuerst; das Original bleibt unverändert.

- **Raw-Details**: setzt die Farben des Sensors neu zusammen (Demosaicing). Feinere Details mit weniger Farbsäumen, Treppen und Moiré, ohne zu glätten. Für rauscharme Bilder, etwa bei niedriger ISO.
- **KI-Entrauschen**: entfernt das Rauschen und erhält dabei feine Details. Für Bilder mit hoher ISO oder stark aufgehellten Schatten.

Der Regler legt jeweils die Stärke fest, mit der das Ergebnis eingemischt wird. Unter dem Regler steht, ob gerade berechnet wird. Bei 100 % KI-Entrauschen ist Raw-Details ohne Wirkung, weil das Entrauschen die Farben selbst neu zusammensetzt.

- Das **A** im Kopf wählt nach dem gemessenen Rauschen: Raw-Details für rauscharme Bilder, KI-Entrauschen für verrauschte.
- Das Übertragen-Symbol im Kopf überträgt beide Regler auf die anderen ausgewählten Bilder.
- Raw-Details gibt es nur für die üblichen Bayer-Sensoren, nicht für Fujifilm X-Trans, lineare DNGs und HDR; der Regler ist dann ausgegraut.
- Fehlt das Ergebnis beim Export, wird es dort berechnet.

Die klassische [Rauschreduzierung](entwickeln#Rauschreduzierung) steht als eigener Abschnitt darunter und lässt sich zusätzlich verwenden.

## Entfernen

**Entfernen** ({taste:tool-remove}) im Tab Retusche: über Störendes malen, beim Loslassen füllt die KI die Stelle passend zur Umgebung. Das Modell wird geladen, sobald das Werkzeug gewählt wird. Bedienung, Pinsel und die Liste der Retuschen stehen unter [Retusche](retusche).

## Super Resolution

**Entwickeln → Super Resolution …** verdoppelt Breite und Höhe eines Bildes (viermal so viele Pixel), etwa für große Drucke oder starke Ausschnitte. Die KI arbeitet feine Details heraus, ohne welche zu erfinden.

1. Ein oder mehrere Bilder auswählen.
2. **Entwickeln → Super Resolution …** wählen; der Dialog zeigt die Zielgröße.
3. **Vergrößern** startet die Berechnung im Hintergrund.

Das Ergebnis ist ein neues DNG neben dem Original, mit „-SR“ im Namen. Weißabgleich, Farbe und Tonwerte bleiben darin voll bearbeitbar, die übrige Bearbeitung wird übernommen.

- KI-Entrauschen und Raw-Details werden so eingerechnet, wie sie unter **Verbessern** eingestellt sind; sonst würde das Rauschen mit vergrößert.
- Ist das Bild zugeschnitten, wird nur der Ausschnitt vergrößert. Geraderichten, Perspektive und Objektivkorrektur werden dann fest eingerechnet, Ebenen und Retusche nicht übernommen.

## Neu ausleuchten

Abschnitt im Tab **Belichtung**. Hellt Vorder- und Hintergrund getrennt auf oder ab und färbt sie wärmer oder kühler, etwa um ein Motiv im Gegenlicht aufzuhellen. Dafür schätzt die KI einmal je Bild, wie weit jede Stelle entfernt ist (Tiefenkarte).

- **Vordergrund** und **Hintergrund** haben je **Belichtung** und **Wärme**.
- **Trennung**: über der Verteilung der Entfernungen, links nah, rechts fern. Die Linie ist die Grenze, die gestrichelten Linien Anfang und Ende des Übergangs; jede lässt sich einzeln ziehen.

## Objektivunschärfe

Abschnitt im Tab **Effekte**. Zeichnet Vorder- und Hintergrund weich wie eine offen fotografierte Blende, etwa für ein Porträt vor unruhigem Hintergrund. Nutzt dieselbe Tiefenkarte.

- **Stärke**: wie unscharf es wird.
- **Fokus setzen**: auf die Stelle klicken, die scharf bleiben soll. Alternativ der Regler **Fokus**. Beim ersten Einschalten liegt der Fokus auf dem nächsten Teil des Bildes, meist dem Motiv.
- **Schärfebereich**: wie viel vor und hinter dem Fokus scharf bleibt.
- **Lichter**: helle Stellen werden in der Unschärfe zu leuchtenden Scheiben (Bokeh).

## Atmosphäre

Abschnitt im Tab **Effekte**. Legt Dunst über das Bild, der mit der Entfernung dichter wird, etwa für Morgennebel in einer Landschaft. Seine Farbe kommt aus der Ferne des Bildes.

- **Stärke**: wie dicht der Dunst ist.
- **Reichweite**: wie weit nach vorn er reicht.
- **Helligkeit** und **Wärme**: verändern seine Farbe.

> Neu ausleuchten, Objektivunschärfe und Atmosphäre wirken erst, wenn die Tiefenkarte berechnet ist; der Abschnitt zeigt den Fortschritt. Fehlt sie beim Export, wird sie dort berechnet.

## Motiv, Himmel und Tiefe

Im Tab **Ebenen** erzeugen die Knöpfe unter **Maske → Automatisch auswählen** eine Maske aus dem Bildinhalt: **Motiv**, **Himmel** und **Tiefe**. Dazu kommt das Werkzeug **Objekt** ({taste:tool-object}), das per Klick ein Objekt auswählt. Die Bedienung steht unter [Ebenen & Masken](ebenen-masken#Automatisch auswählen).

Die genaue Motivmaske braucht einige Sekunden. Zeigt der Tab Ebenen ein Bild, wird sie schon vorab im Hintergrund berechnet; kommt der Klick früher, erscheint sofort eine grobe Maske, die die genaue ersetzt, sobald sie fertig ist.

## Inhaltssuche

Die KI beschreibt jedes Bild einmal als Zahlenreihe, mit der sich Bilder und Texte vergleichen lassen. Damit arbeiten das Feld **Bildinhalt** unter [Filter & Suche](bibliothek#Suche nach dem Bildinhalt), **Ähnliche finden** im Menü **Foto** und die [Keyword-Vorschläge](bibliothek#Keywords) im Tab Metadaten. Erfasst werden die Bilder erst, wenn eine Suche oder die Vorschläge sie brauchen.
