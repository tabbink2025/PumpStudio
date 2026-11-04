# Pump Studio — Windows

Start `pump-studio.exe`. Geen installatie nodig. Bewaar de map `bridge` naast het programma (nodig voor hardware). Gegevens staan in `data\pumpstudio.db` (SQLite), het logboek in `logs\pumpstudio_JJJJMMDD.log`.

## Opzet

Zijbalk met pagina's, kopbalk met profiel, status per apparaat en de rode **STOP**-knop (ook **Esc**).

| Pagina | Wat |
|---|---|
| **Pompen** | Alle pompen van het profiel; filter op tekst, kop, actief. Kop, ingrediënt en actief direct aanpassen (meteen opgeslagen). Handbediening. Kolom Kalibratie = datum van de actieve kalibratie. |
| **Kalibratie** | VSD: Camelot-methode met wachtrij. Keramisch: kalibratie (g/omw) en 360°-meetcurve. Grafieken, kerngetallen, historie, controle doseren. |
| **Pomptest** | Doseertest: één gewicht, reeks of willekeurig; correctie in 0,1 %-stappen; uitslag per gewicht; uitsluiten met reden; historie; Word-verslag en CSV. |
| **Monitors** | Live gewicht per kop, CAN-bus test, PLC-berichten, scannertest. |
| **Logboek** | Elke klik, opdracht, antwoord en fout; filter op tekst, niveau en gebied; open logmap; kopiëren. |
| **Instellingen** | Profiel, verbindingen per apparaat, verbindingsinstellingen, lifthoogte per kop, lijnplekken, scanner, labelprinter, logretentie. |
| **Recepten** | Recepten met geordende stappen (één ingrediënt = na elkaar, meerdere = tegelijk), versies, uitvoeren op één pot met weging per stap, resultaten en pottenhistorie (VSD). |
| **Ingrediënten** | Ingrediënt ↔ pomp/kop, dichtheid, tolerantie, voorkeurskop/-pomp, waarschuwingen, CSV import/export. |
| **Productie** | Orders → potten met eigen barcode → wachtrij; labels; Start/Pauze/Stop/Afbreken/Herstart; lijnbeeld; meldingen; productiehistorie met filters. |
| **Machine/lijn** | Handbediening lijn 1310, transport 1220, liften, units, barcode 1190; vrije PLC-opdrachten; scannertest; PLC initialiseren (zoals Camelot). |
| **Analyse** | Overzicht vandaag / 7 dagen, productie per ingrediënt, pomp en recept, historie per pomp (kalibraties en testen); filters periode, kop, simulatie; export Excel en CSV. |

## Profielen

- **VSD**: 192 pompen (A/B op LH2, borden 610–621; C/D op LH1, borden 622–633; 8 motoren per bord), weegschalen LH1 (192.168.0.56:23) en LH2 (192.168.0.55:23), PLC 10.6.1.15.
- **Keramisch**: pompbank met één keramische pomp (INV-13/INV-36) en weegschaal (COM of TCP).

Wisselen in de zijbalk of bij Instellingen, zonder herstart. Lopende opdrachten worden eerst gestopt en de apparaten van het oude profiel verbroken; het scherm blijft bedienbaar ("Profiel wisselen…"), nieuwe opdrachten worden zolang geweigerd.

Bij overstap van iteratie 1: sluit de oude app vóór de eerste start van de nieuwe (de database krijgt dan schemaversie 2; posities worden omgerekend naar stappen).

## Verbinden

Instellingen → **Verbindingen**: elk apparaat apart verbinden/verbreken, per apparaat **Simulatie** of **Hardware**.

- Simulatie en hardware worden niet gemengd (alleen met het vinkje "tegelijk toestaan (alleen voor tests)", dan rode waarschuwing in de kopbalk).
- Eén verbinding per apparaat: dezelfde weegschaal, hetzelfde CAN-kanaal of dezelfde PLC-poort kan niet twee keer open; de CAN-test claimt alle kanalen zolang hij loopt.
- Elk apparaat heeft een eigen achtergrondtaak: een trage of stille weegschaal houdt pompen en PLC niet op.
- Een weggevallen weegschaalverbinding wordt rood gemeld.
- Hardware: Camelot en de bestaande CAN-app eerst sluiten (zelfde CAN-borden, PLC-kanaal en weegschalen). Nodig: Python met python-can/IXXAT (standaard `C:\Users\Thomas\CAN\.venv\Scripts\python.exe`).

## VSD-pompen bedienen (Pompen)

Positie in **stappen**: 51 200 stappen per omwenteling (per pomp instelbaar). Invoer in stappen (standaard), omwentelingen of graden; de omrekening staat er altijd bij, bv. `25 600 st = 0,5000 omw = 180,0°`.

- Stappen: geheel getal; spatie of punt als duizendtalscheiding (`25 600`, `25.600`). Komma of decimalen worden geweigerd met melding (`25,6` of `1.5` in stappen is fout).
- Omwentelingen en graden: komma of punt als decimaal (`0,5`, `0.5`).
- Naar de drive gaat p1 = doel, p2 = hele omwentelingen en p3 = rest, afgekapt richting nul zoals Camelot (−12 800 st → p2 0, p3 −12 800).

- **Doseren (+)**: relatief vanaf de laatst bekende positie, alleen vooruit. Doel p1 = start + stappen, exact naar de drive. Daarna zet de pomp zijn teller met **SETPOS** terug naar de fase binnen de slag (zoals Camelot): de absolute positie springt dus terug. Het resultaat toont positie voor, doel, bereikt en fase na SETPOS.
- **Naar positie**: absolute positie (MOVETO), mag terug na bevestiging.
- Home, Zero, Init, Status. Gekozen pompen draaien tegelijk.
- Bereik: geheel getal binnen het 32-bit bereik van de drive; negatief doseren wordt geweigerd.

## Keramische pomp bedienen (Pompen)

Eerst **Huidige stand als startpositie 0** of **Home via sensor**. Alleen vooruit (relatief in stappen/omw/graden, naar teller, naar fase), maximaal 100 omwentelingen per opdracht, optioneel wegen voor en na. De app telt zelf door; boven de controllergrens (standaard 10 000 000 stappen) volgt een bevestigde ZERO zonder beweging (tellerbeveiliging).

## Kalibratie

- **VSD**: kies een pomp of zet meerdere pompen in de wachtrij (gekozen op Pompen). Fase 1: 41 punten tot 210° van één slag, 2 runs; fase 2: 1, 2, 3, 5, 7, 10, 13, 16 omwentelingen, 2 runs; 2 s wachten per weging (Camelot-standaard, aanpasbaar). Weging op de weegschaal van de **eigen kop**; zonder die weegschaal of bij een pomp die uit staat wordt geweigerd. Voortgang per stap; STOP onderbreekt binnen 1 s.
- **Keramisch**: kalibratie (omwentelingen × herhalingen, optioneel prime) en 360°-meetcurve (per hoek of per aantal stappen).
- Opslag direct in de database: nieuwste geslaagde kalibratie = **actief** per pomp, soort én modus (simulatie/hardware), oudere van dezelfde modus = **archief**; gestopt of mislukt = **afgebroken** (nooit actief). Een simulatie-kalibratie vervangt nooit een hardware-kalibratie. Historie met modus en afwijking t.o.v. de vorige.
- **Controle doseren** (standaard 1 g): op hardware alleen met een hardware-kalibratie (anders geweigerd); in simulatie met de simulatie-kalibratie, of de hardware-kalibratie als er geen simulatie-kalibratie is. Resultaat wordt bij de kalibratie bewaard. Pompen toont de datum van de actieve kalibratie, met "(sim)" als er alleen een simulatie-kalibratie is.
- Bij de eerste start worden oude bestanden `machine\calibration\*.json` en `results\characterization_*.json` eenmalig overgenomen; de bestanden blijven staan.

## Pomptest

- **Opzet**: pomp kiezen (VSD: actieve pomp, weging op de eigen kop; keramisch: de pompbank). Modus *Eén gewicht* × n, *Reeks* (bv. `0,5; 1; 2; 3; 4; 5; 10; 15; 20` × n, oplopend of door elkaar) of *Willekeurig* (n doses tussen min en max). Bij door elkaar/willekeurig staat het **zaad** erbij: zelfde zaad = dezelfde gewichten. Tolerantie (standaard ±1 %), wachttijd voor stabiel gewicht, tarra per dosis (VSD; uit = weging na de vorige dosis als nulpunt), pauze na elke dosis (knop *Volgende dosis*).
- **Kalibratie**: altijd de actieve kalibratie in de juiste modus; hardware zonder hardware-kalibratie wordt geweigerd. Keramisch: doseert vanuit de 360°-meetcurve van de huidige sessie (eerst Kalibratie → Meetcurve na startpositie 0).
- **Uitvoeren**: dosis voor dosis, elke dosis direct opgeslagen; live tabel en grafiek. STOP / Esc stopt binnen 1 s; de test blijft bewaard als *gestopt*.
- **Correctie** −20 … +20 % in stappen van 0,1 %, ook tussen doses; geldt vanaf de volgende dosis en staat bij elke dosis. *Aanbevolen* = per dosis de correctie die precies het gevraagde gewicht had gegeven, gemiddeld over de laatste n doses (doses van 0 g of minder worden overgeslagen en genoemd); alleen met *Overnemen*.
- **Pompinstellingen** (VSD: snelheid, acceleratie, deceleratie) tussen doses aan te passen zonder opnieuw te verbinden; keramisch via Instellingen → Toepassen. Per dosis vastgelegd.
- **Uitslag** per gewicht: gemiddelde afwijking (% en g), spreiding, kleinste/grootste afwijking, aantal binnen tolerantie, g/min. Uitschieters worden nooit automatisch verwijderd; *Uitsluiten…* vraagt een reden, de dosis blijft zichtbaar (doorgestreept) en telt niet mee. Keramisch: spreiding bij kleine doses als indicatie van de druppelgrootte.
- **Word-verslag** (één of meer testen van dezelfde pomp, aanvinken in Historie): inleiding, tabel per gewicht, in te vullen conclusie en advies, grafiek en alle doses. Simulatie staat in de titel; simulatie en hardware nooit samen. **CSV** in Nederlandse notatie (puntkomma, decimale komma) voor Excel. Beide in de map `results`.
- Oude doseertesten (`results\dosering_*.json`, `doseertest_*.json`) worden bij de eerste start eenmalig overgenomen; de bestanden blijven staan.

## Ingrediënten

- De koppeling ingrediënt ↔ pomp staat per pomp op **Pompen** (kolom Ingrediënt); de pagina Ingrediënten toont per ingrediënt de pompen en hun kop. Hier: naam, dichtheid, eigen tolerantie (leeg = die van het recept), voorkeurskop en voorkeurspomp (als meer pompen hetzelfde ingrediënt hebben).
- **Waarschuwingen**: ingrediënt zonder pomp, pomp uit, pomp niet gekalibreerd in de huidige modus (hardware vraagt een hardware-kalibratie), pompen zonder ingrediënt.
- **Verwijderen** wordt geweigerd als een recept het ingrediënt gebruikt (de recepten staan in de melding); anders worden de pompen ontkoppeld.
- **CSV** (puntkomma, decimale komma): `Naam;Dichtheid (g/ml);Tolerantie (%);Voorkeurkop;Opmerking;Pompen` (de kolom Pompen is alleen informatie; de koppeling blijft op Pompen). Importeren voegt toe of werkt bij op naam; één foute regel weigert het hele bestand met het regelnummer.

## Recepten

- **Editor**: stappen omhoog/omlaag, kopiëren, verwijderen; per stap ingrediënten met gram, eventueel eigen tolerantie en kop (standaard *auto*: de kop van de pomp). Een stap met één ingrediënt doseert na de vorige; een stap met meer ingrediënten doseert die tegelijk (alle drives in één CAN-blok). *Bijdoseren* per stap: één keer extra als een ingrediënt onder de ondergrens blijft. Te veel wordt gemeld, nooit gecorrigeerd.
- **Validatie** bij het typen (leeg recept, lege stap, 0 of negatief gram, twee keer hetzelfde in een parallelle stap, onbekend ingrediënt) en bij uitvoeren (ingrediënt zonder pomp, pomp uit of niet gekalibreerd in de modus van de CAN-verbinding).
- **Versies**: opslaan van een recept dat al op een pot gedraaid heeft maakt een nieuwe versie; elke pot verwijst naar zijn versie. Oude versies zijn terug te laden in de editor.
- **Uitvoeren op één pot**: nodig zijn CAN, PLC en de weegschaal van elke kop in het recept. Plan per kop zichtbaar. Na **Start pot** (echte machine: eerst bevestigen) gaat unit + lift van de kop omhoog via de PLC, wordt per stap gewogen en gaat de lift omlaag aan het eind, bij STOP en bij een fout. Recept op beide koppen: eerst LH1, daarna de pot onder LH2 zetten en **Volgende kop** klikken. De lift gaat omhoog met P1 = hoogte × 10 (zoals Camelot): de **pothoogte** van het recept, anders de **lifthoogte per kop** uit Instellingen (VSD). Op hardware start een pot niet zonder hoogte; in simulatie geldt 10 cm. PLC-antwoorden zoals Camelot: lift Start 2/4, lift Stop 9, unit Start 2, unit Stop 9. Antwoord 16 (geen pot) of een ander antwoord: die kop doseert niet, de lift gaat omlaag en de pot krijgt *fout* met de reden. Elke kop werkt in zijn eigen ritme met zijn eigen CAN-blok en weging; een fout op de ene kop raakt de andere kop niet.
- **Resultaat** per ingrediënt: gevraagd, gedoseerd, afwijking, oordeel (binnen tolerantie / te weinig / te veel), bijgedoseerd, *toegerekend* (parallel gedoseerd: het nettogewicht van de kop is naar verhouding van de gevraagde grammen verdeeld). Gemeten waarden worden nooit aangepast.
- **STOP / Esc** tijdens een pot: pompen stoppen, lift omlaag, de pot blijft bewaard als *gestopt* met wat tot dan gewogen is. Een pot die liep toen de app sloot krijgt bij de volgende start de status *onderbroken*.

## Productie

- **Order** = receptversie × aantal potten (+ ordernaam, batch, prioriteit −9…9; hoger = eerder). Elke pot krijgt een record met barcode = pot-id in 10 cijfers (zoals Camelot, geen controlecijfer); uniek in de hele database. Eigen (voorgedrukte) code koppelen kan per wachtende pot (*Code…*). Een gebruikte of afgeronde code wordt nooit opnieuw gedoseerd.
- **Labels**: *Labels* bij de order maakt een labelvel (HTML, Code 128 als SVG) in `results\labels` en opent het in de browser om af te drukken. Met een labelprinter in Instellingen (IP, poort 9100) stuurt *ZPL* de labels rechtstreeks.
- **Controle vooraf** (staat op de pagina): PLC, CAN, beide weegschalen en de scanner verbonden; alle pompen van de wachtende recepten actief en gekalibreerd in de modus van de CAN-verbinding; op hardware een lifthoogte per kop. Anders start de lijn niet en staat er wat ontbreekt.
- **Cyclus na Start** (zoals Camelot *UnitTrainSync*, met time-outs): lijn 1310 Start → herhalen: doseren (LH1 en LH2 tegelijk, elk een eigen pot, elk in eigen ritme) → nieuwe pot scannen (1190 Start P1 = 1 + scanner; 2 = pot, 16 = geen pot; no-read: Stop + Reset en opnieuw, standaard 2 pogingen) → transport 1220 één plek (alleen met beide liften omlaag) → tot de lijn leeg is en de wachtrij leeg of Stop → lijn 1310 Stop.
- **No-read / onbekende code / gebruikte code**: de pot gaat ongedoseerd mee (*niet gedoseerd*) of — instelbaar — Hold met vraag aan de operator (code invoeren en Herstart). **Geen pot** (16): plek blijft leeg. Alles staat in de meldingen en de historie.
- **Pauze**: lopende dosering maakt af, geen nieuwe start of transport. **Stop**: geen nieuwe potten, de lijn loopt leeg. **Afbreken** (ook STOP/Esc): pompen stoppen, liften omlaag, Abort (5) naar lijn, transport, barcode en units; potten blijven op hun plek bekend als *afgebroken*.
- **Twee koppen tegelijk**: de doseerblokken van LH1 en LH2 lopen echt gelijktijdig over de CAN-bus (elk blok wacht alleen op zijn eigen pompen); een drivefout raakt alleen het blok van die kop.
- **Liften**: bij Start, *Doorgaan met deze lijn* en *Herstart* krijgen beide liften eerst Stop en moeten ze 9 (omlaag) antwoorden; anders Hold. Start wordt geweigerd als een lift met de hand omhoog staat. Transport alleen met beide liften omlaag; antwoord 16 op transport = niet uitgevoerd (lijnbeeld blijft, Hold).
- **Fout** (PLC-foutantwoord of time-out, weegschaal, pomp, geen pot onder de lift): Hold met de oorzaak (Hold 6 naar de lijn); *Herstart* (Restart 7) probeert de stap opnieuw, *Afbreken* stopt. Een kop die mislukt maakt alleen die pot *fout*; de andere kop gaat door.
- **Herstart van de app** tijdens productie: lijnbeeld en potstatus staan in de database. Een pot die aan het doseren was wordt *onderbroken* en nooit automatisch opnieuw gedoseerd. De operator kiest *Doorgaan met deze lijn* of *Lijn leegmaken*.
- **Lijnplekken** (Instellingen → VSD): aantal tussenplekken scan → LH1, LH1 → LH2 en na LH2 (standaard zoals Camelot: scan → wacht → LH1 → LH2 → uit).

## Machine/lijn en scanner

- Handbediening alleen na een klik, niet tijdens productie of een pot; op een echte PLC eerst bevestigen. Transport wordt geweigerd als een lift omhoog staat. Lift omhoog gebruikt de lifthoogte van Instellingen (P1 = hoogte × 10).
- **PLC initialiseren (zoals Camelot)**: per module Abort (5), PowerUp (98), Reset (2). Nooit automatisch; alleen met bevestiging dat Camelot niet draait.
- **Scanner** (Instellingen → VSD): netwerk (TCP) of COM-poort (via `bridge\scanner_bridge.py`, pyserial). Generiek protocol zoals Camelot: trigger `BA`, antwoord `<STX>code<ETX><CR><LF>`, `<STX><NAK>` = geen code, trigger uit `B`; 10 s, 2 pogingen. *Scanner testen* op Machine/lijn en Monitors toont het ruwe antwoord. In simulatie geeft de scanner de code van de volgende wachtende pot.

## Analyse

- **Selectie**: periode (vandaag … 1 jaar, alles), kop (beide/LH1/LH2) en *simulatie meetellen*. Simulatie staat standaard **uit**; aan = rode balk boven de cijfers, kolom *Soort* in de export en `_met_simulatie` in de bestandsnaam. Berekenen gebeurt op de achtergrond (het scherm blijft bedienbaar).
- **Overzicht**: aantal klaar-potten vandaag (sinds 0:00) en de laatste 7 dagen, % binnen tolerantie, de 5 slechtste ingrediënten en pompen (gemiddelde absolute afwijking, laatste 7 dagen).
- **Productie per ingrediënt / per pomp**: n, gemiddelde afwijking, gemiddelde absolute afwijking, spreiding, aantal binnen tolerantie. Telt mee: gewogen ingrediënten met een oordeel (binnen tolerantie, te weinig, te veel); niet gewogen of gestopt telt niet. Parallel gedoseerde (toegerekende) waarden tellen mee; hun aantal staat erbij.
- **Potten per recept**: aantal potten en % binnen tolerantie. Een pot is binnen tolerantie als hij *klaar* is en al zijn gewogen ingrediënten binnen hun tolerantie vallen (met kopfilter: alleen die kop).
- **Historie per pomp**: g/slag (of g/omw) van alle kalibraties door de tijd, ook archief en geïmporteerde; pomptest-afwijking per gewicht door de tijd met n, gemiddelde en spreiding. Uitgesloten testdoses tellen nooit mee.
- **Export** van de selectie (map `results\analyse`): **Excel** (.xlsx, tabbladen Potten, Ingrediënten, Testen, Kalibraties; getallen als getal, datums als datum) of **CSV** (vier bestanden, puntkomma, decimale komma, UTF-8 met BOM).

## Instellingen

- **Profiel** en **Verbindingen** (zie boven). **Logboek**: logregels in de database bewaren (standaard 90 dagen, 0 = alles); opruimen bij het starten, op de achtergrond. De dagbestanden in `logs` blijven staan.
- **VSD**: CAN-kanaal en bitrate, Python-pad, weegschalen LH1/LH2 (IP en poort), PLC (IP, poorten, FINS-nodes, kanaal, time-out), **lifthoogte per kop** (P1 = hoogte × 10; leeg = niet ingesteld), **lijnplekken** (tussenplekken scan → LH1, LH1 → LH2, na LH2; no-read: ongedoseerd mee of vragen), **barcodescanner** (TCP of COM, triggerteksten, time-out, pogingen) en **labelprinter** (ZPL, IP en poort 9100, optioneel).
- **Keramisch**: pomptype, stappen/omw, CAN-node/motor, snelheid, versnelling, slaginstelling, vloeistof, controllergrens, wegen (venster, variatie, time-out, uitlooptijd), weegschaal COM of TCP.
- *Toepassen* slaat op in de database; geldt bij opnieuw verbinden.

## Op de machine controleren (eerste keer met hardware)

- **Camelot** gesloten (zelfde CAN-borden, PLC-kanaal, weegschalen en scanner).
- **Lifthoogte** per kop (Instellingen) of pothoogte per recept: controleer met *Lift omhoog* op Machine/lijn dat de pot op de juiste hoogte komt (P1 = cm × 10).
- **Scanner**: welk type en welke poort (TCP of COM); *Scanner testen* moet `<STX>code<ETX>` tonen.
- **PLC-belasting**: Pump Studio leest het postvak elke 15 ms tijdens een opdracht; kijk of de PLC dat zonder problemen bijhoudt. Bij twijfel *PLC initialiseren (zoals Camelot)* alleen na overleg.
- **Unit-antwoord na STOP**: na STOP/Afbreken gaat unit Stop zonder wachten; kijk in Monitors → PLC-berichten of daarna toch 9 komt.
- **Liften na STOP**: beide liften moeten omlaag gaan (PLC-antwoord 9).
- **Weegschalen**: stabiele weging binnen de time-out; pc in 192.168.0.x.
- Eerst één pot met een eenvoudig recept (Recepten → Uitvoeren), dan pas productie.

## Logboek en STOP

Alles staat in het dagbestand en op de pagina Logboek (ook geïndexeerd in de database). **STOP / Esc** onderbreekt elke lopende pompopdracht, kalibratie, CAN-test, stabiele weging en PLC-wachtstap; Esc in een tekstveld of open keuzelijst sluit alleen dat veld. Een STOP terwijl niets loopt laat de keramische referentie geldig. Softwarestop hangt af van werkende CAN/driver: gebruik de fysieke noodstop waar nodig.

PLC-diagnose (Monitors): op een echte PLC alleen Reset en Abort; lijn, transport, liften en vrije opdrachten op Machine/lijn (met bevestiging).

## Bouwen en testen

Rust (stable) en Visual C++ build tools.

```powershell
cargo test --locked
cargo build --release --locked
```

Offline controle zonder venster en zonder hardware (database en resultaten in de opgegeven map):

```powershell
.\pump-studio.exe --smoke-test C:\temp\pumpstudio-smoke
```

De smoke-test tekent alle pagina's in beide profielen, verbindt alle apparaten in simulatie, doseert in stappen, kalibreert (VSD en keramisch), voert pomptesten uit (reeks, correctie tussen doses, STOP), maakt een verslag en CSV, draait een recept op één pot met de PLC-emulator (parallelle stap in één CAN-blok, STOP met lift omlaag), draait productie met 3 orders / 7 potten op de gesimuleerde lijn breekt een productie af met STOP, berekent de analyse en exporteert Excel en CSV, en test STOP, controle doseren en de profielwissel. Uitkomst in `smoke-ok.txt` of `smoke-error.txt` in die map.

Bridge-tests (offline, virtuele CAN-bus en nep-COM-poort): `cd bridge; python -m unittest test_vsd_bridge test_scanner_bridge` (de xlsx-test in `cargo test` leest het bestand ook met openpyxl uit `C:\Users\Thomas\CAN\.venv` als die er is; alleen lezen).
