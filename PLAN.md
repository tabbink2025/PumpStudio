# Pump Studio — plan (bindend)

Opgesteld na interview met Thomas (2 november 2026). Dit document is de bron waartegen de Critic beoordeelt. Wijzigingen alleen via een nieuwe iteratiesectie of een addendum.

## Doel

Eén functionele desktop-app (Windows, één .exe) voor het testen, kalibreren en productief gebruiken van doseerpompen, in de geest van de CAN-app (Octopus): duidelijke pagina's, alles los te bedienen, betrouwbaar en vrijer in gebruik dan Camelot. Twee opstellingen:

- **VSD-machine**: koppen LH1 en LH2, 192 VSD-membraanpompen op CAN-borden (node 610–633, 8 motoren per bord), Mettler-weegschalen per kop via TCP (MT-SICS), liften/units/lijn/transport/barcode via Omron NJ-PLC (FINS/UDP-postvak zoals Camelot).
- **Keramische pompbank**: keramische pomp (INV-13/INV-36) via CAN, één of meer weegschalen.

Gebruikers: engineer (testen/kalibreren), operator (productie), service (diagnose). **Geen rollen**: iedereen ziet alles.

## Kwaliteitslat

1. **Functioneel vóór mooi**: elke functie werkt aantoonbaar (test of simulatie), geen knoppen die niets doen.
2. **Nooit vastlopen**: geen hardware-aanroep blokkeert de UI; elke wachtstap heeft een time-out; STOP/Esc werkt altijd en overal.
3. **Veilig**: keramische pomp alleen vooruit, met tellerbeveiliging; VSD-pompen alleen met geldige parameters; liften/transport alleen na expliciete klik; nooit tegelijk een gesimuleerd en een echt apparaat door elkaar.
4. **Eén verbinding per apparaat**: een weegschaal, CAN-kanaal of PLC wordt maar door één onderdeel tegelijk geopend; conflicten worden gemeld, niet stil genegeerd.
5. **Navolgbaar**: elke klik, opdracht, antwoord en fout staat in het **Logboek** (dagbestand + pagina met filter). Behouden van huidige app.
6. **Data veilig**: SQLite als bron; geen dataverlies bij afbreken; elke meting/kalibratie/test direct opgeslagen.
7. **Simuleerbaar**: elke hardwarefunctie heeft een simulatievariant, zodat Critic en gebruiker alles kunnen testen zonder machine (Planner-besluit: nodig voor de review, ook al is simulatie niet als "must" gekozen).
8. **Stijl**: licht, zoals de huidige app, maar consistent: zijbalk met pagina's, kaarten, één kleurenpalet, leesbaar bij 1366×768.
9. **Nederlands** in de UI, kort en zakelijk.

## Architectuur (Planner-besluit)

- Rust + egui blijft (gekozen). Nieuwe UI-schil; bestaande hardwarecode wordt hergebruikt en waar nodig opgeschoond:
  `plc.rs` (FINS-postvak), `machine.rs` (VSD-CAN, weegschalen), `vsdcal.rs` (VSD-kalibratie), `model.rs`/`workflow.rs` (keramische curve, teller), `bridge/` (Python CAN/IXXAT), `applog.rs` (logboek), `report.rs` (Word).
- **Opslag: SQLite** (nieuwe dependency `rusqlite` met gebundelde SQLite). Tabellen minimaal: `profile`, `settings`, `pump`, `ingredient`, `calibration` (+ punten), `test_run` (+ doses), `recipe`, `recipe_step`, `recipe_item`, `production_run` (+ per ingrediënt), `log` (index; tekst blijft ook in dagbestand). Export naar CSV/Excel vanuit de app.
- **Profielen** in Instellingen: actief profiel = VSD of Keramisch (wisselen zonder herstart). Pagina's tonen alleen wat bij het profiel hoort.
- Eén achtergrondtaak per apparaatgroep (pompen/CAN, weegschalen, PLC), zodat een trage weegschaal nooit de lift of de pompen blokkeert.

## Pagina's (zijbalk)

| Pagina | Inhoud |
|---|---|
| **Pompen** | Lijst alle pompen van het profiel (naam, kop, CAN-adres, ingrediënt, aan/uit, positie, kalibratiestatus); filter; handbediening per pomp of groep (VSD: dispense/move/home/zero/init/status; keramisch: vooruit naar stappen/omw/fase, wegen voor/na). |
| **Kalibratie** | VSD: Camelot-methode (fase 1 + fase 2) op weegschaal van eigen kop. Keramisch: forward-only kalibratie (+ prime) en 360°-meetcurve. Resultaten, grafieken, historie. |
| **Pomptest** | Doseertest per pomp: één gewicht, reeks, of n willekeurig tussen min/max; correctie in 0,1 %-stappen; resultaten per gewicht (gemiddelde, spreiding, min/max, binnen ±1 %). |
| **Recepten** | Recepten maken: geordende **stappen**; een stap is één ingrediënt (sequentieel) of een groep (parallel); stappen mogen elkaar afwisselen; recept mag over LH1 en LH2 verdeeld zijn (kop volgt uit pomp van het ingrediënt). Recept uitvoeren op één pot (met lift) en per stap wegen. |
| **Ingrediënten** | Ingrediënt ↔ pomp/kop, naam, eventueel dichtheid en opmerkingen. Waarschuwing bij ingrediënt zonder pomp of dubbele koppeling. |
| **Productie** | Orderwachtrij, barcodes/labels, Start/Pauze/Stop/Afbreken, lijnbeeld; zie Productieproces. |
| **Machine/lijn** | Handbediening VSD: lijn start/stop, transport een plek verder, liften, units, barcode; lijnstatus (welke pot waar). |
| **Monitors** | Live weegschalen (beide koppen), CAN-bus test/scan, PLC-berichten. |
| **Analyse** | Historie per pomp (kalibraties, tests), productiehistorie (recepten/potten, afwijking per ingrediënt), export CSV/Excel. |
| **Logboek** | Zoals nu: dagbestand + filter op tekst/niveau, open logmap, kopiëren. |
| **Instellingen** | Profiel, verbindingen (CAN-kanaal/bitrate, PLC-IP/nodes/kanaal, weegschalen COM of TCP per kop), pomptypes, tellergrens, time-outs. |

## Receptmodel (bindend)

- `recipe` → geordende `recipe_step` → per stap één of meer `recipe_item` (ingrediënt, gram).
- Stap met één item = sequentieel; stap met meerdere items = parallel (alle pompen tegelijk starten, wachten tot allemaal klaar).
- Items van één stap kunnen op beide koppen liggen; per kop wordt apart getarreerd en gewogen.
- Gewichtstoewijzing: sequentieel = netto gewicht van dat ingrediënt; parallel per kop = netto van de kop verdeeld naar verhouding van de gevraagde gewichten (zoals Camelot), duidelijk als zodanig gemarkeerd in resultaat.
- Ingrediënt zonder (actieve) pomp of ongekalibreerde pomp → recept start niet, met melding welke.
- Te weinig en te veel worden beide gemeld (Camelot-fout niet herhalen).

## Productieproces (bindend, na onderzoek Camelot-code 2 nov 2026)

Doel: operator zet orders klaar, drukt op **Start**, en de lijn draait zelf tot de wachtrij leeg is.

**Nieuwe pagina "Productie"** (VSD-profiel): orderwachtrij, barcodes/labels, knoppen Start / Pauze / Stop / Afbreken / Herstart na fout, lijnbeeld (welke pot op welke plek, status per kop), voortgang per pot.

**Order en pot**
- Order = recept × aantal potten (+ optioneel ordernaam/batch, prioriteit). Elke pot krijgt een eigen record met unieke barcode.
- Barcode = door Pump Studio gemaakt, standaard 10 cijfers met voorloopnullen van het pot-id (zelfde vorm als Camelot, geen controlecijfer). Codes zijn uniek over de hele database; een al gebruikte of afgeronde code wordt nooit opnieuw gedoseerd.
- Labels: zie open beslissing B3.

**Scanner** (Camelot leest de scanner zelf; de PLC geeft geen barcode door)
- Directe verbinding COM of TCP, in te stellen in Instellingen. Protocol "generiek" zoals Camelot: trigger aan `BA`, uit `B` (instelbaar), antwoord `<STX>code<ETX><CR><LF>`, no-read `<STX><NAK>`. Time-out 10 s, 2 pogingen (instelbaar).
- Tegelijk PLC-proces 1190 Start (P1=1): antwoord 2 = pot op scanplek, 16 = geen pot. Na lezen Stop(8) → 9. Herpoging: Stop(8) + Reset(2), dan opnieuw.
- Losse "scanner testen"-knop op Monitors (trigger, ruwe tekst tonen).
- Welke scanner en poort op de machine zit is nog niet bekend (Camelot-DB S06 nu niet bereikbaar): uitzoeken bij start iteratie 5.

**Cyclus na Start** (zoals Camelot "Sync", maar met time-outs en zonder potten te vergeten)
1. Controle vooraf: PLC, CAN, beide weegschalen en scanner verbonden; alle pompen in de wachtrij-recepten actief en gekalibreerd. Anders start niet, met melding wat ontbreekt.
2. Lijn 1310 Start(1).
3. Lus:
   a. Doseren: elke kop met een pot erop (LH1 en LH2 tegelijk, elk een eigen pot) voert zijn deel van het recept uit: unit Start → lift omhoog → tarreren → stappen sequentieel/parallel → lift omlaag → unit Stop. Wachten tot beide koppen klaar zijn.
   b. Aanvoer: wachtrij niet leeg en geen Stop → pot op scanplek scannen en koppelen aan volgende pot uit de wachtrij (gescande code moet een wachtende pot zijn).
   c. Transport 1220 één plek (P1..P6=0, P9=-1, P10=1234567890): Start → 2 (16 = geen pot) → Stop → 9. Alleen als beide liften omlaag zijn.
   d. Lijn leeg en (wachtrij leeg of Stop) → klaar: lijn 1310 Stop(8).
- **Geen pot / no-read / onbekende code**: plek blijft leeg resp. pot gaat ongedoseerd mee en wordt als "niet gedoseerd" vastgelegd (instelbaar: vragen aan operator).
- **Pauze**: lopende dosering maakt af, geen nieuw transport/start.
- **Stop**: geen nieuwe potten, lijn loopt leeg, daarna klaar.
- **Afbreken**: pompen stoppen, alle liften omlaag, Abort(5) naar lijn/units; potten blijven op hun plek bekend met status "afgebroken" (niet vergeten zoals Camelot).
- **Fout** (PLC-foutantwoord, time-out, weegschaal, pomp): cyclus houdt stil (Hold), melding met oorzaak, operator kiest Herstart of Afbreken. Elke PLC-wachtstap heeft een time-out (Camelot heeft er geen).
- Lijnbeeld en potstatus staan in de database, zodat na herstart van de app duidelijk is welke pot waar staat.
- Na verbinden PLC: per module Abort(5), PowerUp(98), Reset(2) zoals Camelot (pas na bevestiging dat dit veilig is op deze machine, zie B4).
- Alles in Logboek; per pot resultaat per ingrediënt in productiehistorie.

## Iteraties

Elke iteratie: Planner-brief → Worker bouwt + tests → Critic (en waar nodig UX-review) → alleen bij PASS opleveren. Thomas test na elke oplevering; zijn feedback wordt addendum.

# Iteratie 1 — Basis: schil, profiel, data, pompen, verbindingen

**Scope**
- Nieuwe UI-schil met zijbalk (alle pagina's zichtbaar; nog niet gebouwde pagina's tonen "volgt in iteratie N").
- SQLite: schema + migratie-versie; tabellen `profile`, `settings`, `pump`, `ingredient`, `log`-index. Eerste start: VSD-profiel met 192 pompen (A/B = LH2, C/D = LH1, node/motor zoals Camelot) en Keramisch profiel met één pomp.
- **Instellingen**: profielkeuze, verbindingen per apparaat, opslaan in DB.
- **Pompen**: lijst + filter + bewerken (kop, ingrediënt, aan/uit) + handbediening (VSD en keramisch), simulatie en hardware.
- **Monitors**: live gewicht per kop, CAN-bus scan, PLC-berichten.
- **Logboek** overgenomen.
- Verbindingsbeheer: één verbinding per apparaat, apart verbinden/verbreken per apparaat (CAN, weegschaal LH1, weegschaal LH2, PLC), status in kopbalk.

**Buiten scope**: kalibratie, tests, recepten, lijn, analyse, rapport.

**Acceptatie**
- A1 Start zonder bestaande DB maakt schema en beide profielen; tweede start behoudt wijzigingen.
- A2 Profiel wisselen toont direct de juiste pompen en verbindingen, zonder herstart.
- A3 In simulatie: pomp(en) bedienen, gewicht live zien, PLC-bericht zien — alles in Logboek.
- A4 Een weegschaal die niet antwoordt blokkeert niets anders (UI, PLC, pompen blijven bedienbaar); getest met een nep-weegschaal die nooit antwoordt.
- A5 Dezelfde weegschaal of hetzelfde CAN-kanaal twee keer openen wordt geweigerd met melding.
- A6 Gesimuleerde en echte apparaten nooit gemengd zonder melding.
- A7 STOP/Esc onderbreekt elke lopende pompopdracht binnen 1 s.
- A8 `cargo test` groen, release-build zonder nieuwe waarschuwingen, smoke-test groen.

## Iteratie 1 addendum (na Critic-review, FAIL)

- R1 Elke nieuwe opdracht die de gebruiker start (ook verbinden, scannen) zet eerst de stop-vlag van zijn groep terug. Een STOP/Esc of profielwissel mag een latere verbinding nooit blokkeren.
- R2 Elke opening van een fysiek kanaal (ook een hardware-CAN-scan) claimt dat kanaal in het register zolang het open is. Profielwissel stopt eerst alle lopende opdrachten (ook scans) en wacht tot ze klaar zijn, of weigert met melding.
- R3 Database-aanmaak (schema + seed) is één transactie; een half aangemaakte DB herstelt bij de volgende start.
- R4 Een afgebroken opdracht meldt "gestopt", nooit "geslaagd".
- R5 Een weegschaal waarvan de verbinding wegvalt, toont status fout (rood) en geeft zijn claim vrij.
- R6 Esc betekent STOP, behalve wanneer een tekstveld of keuzelijst focus heeft (dan sluit/annuleert Esc alleen dat veld, zonder op te slaan). De STOP-knop werkt altijd. Een STOP terwijl er niets loopt maakt de keramische referentie niet ongeldig.
- Planner-besluit P5: de PLC-diagnose op Monitors mag op een echte PLC tot iteratie 5 alleen Reset(2) en Abort(5) sturen; vrije opdrachten alleen in simulatie.
- Planner-besluit P6: DB-schrijfacties van posities in één transactie per opdracht; fouten worden gelogd, niet genegeerd.
- Iteratie 1 PASS (Critic re-check, 2 nov 2026) + STOP-race gefixt (stop-vlag op aanroepende thread gereset; test `stop_right_after_click_is_never_lost`); 66 tests, 0 waarschuwingen; opgeleverd als pump-studio.exe.
- Carry-over naar iteratie 2: profielwissel asynchroon (nu max 3 s UI-blok); Esc in "Kies"-menu ingrediënt geeft STOP; LEESMIJ.md bijwerken naar nieuwe opzet; testmappen `%TEMP%\pump-rust-*` opruimen in oude workflow-tests.
- Te controleren op machine: idle STOP stuurt INIT naar eerder aangestuurde drives — onschadelijk?
- Carry-over naar iteratie 5: oude machine/PLC-paden in `workflow::Engine` opruimen; `vsd_bridge.py wait()` stop-bewust maken en na STOP op de machine testen; `Devices::status()` niet per aanroep kopiëren.

# Iteratie 2 — Kalibratie (VSD + keramisch) + positie in stappen

Feedback Thomas na iteratie 1 (2 nov 2026): "een positie wordt in stappen bepaald bij de vsd, deze is voor 1 omwenteling 51200, dus ik moet dit wel kunnen invullen als ik een pomp naar een positie stuur." Kijk goed naar de manier van doseren.

**Scope**
1. **Positie in stappen (VSD, Pompen-pagina)**:
   - Invoer voor Doseren (relatief) en Naar positie (absoluut) in **stappen** (geheel getal), met eenheidskeuze stappen / omwentelingen / graden; standaard stappen. Altijd beide getoond (bv. `25 600 st = 0,5000 omw = 180,0°`).
   - Stappen per omwenteling per pomp (standaard 51 200, uit instellingen/pompregel), nooit hardcoded in de UI.
   - Positiekolom en resultaat tonen stappen én omwentelingen; na Doseren ook de fase-positie na SETPOS (dosering eindigt met SETPOS naar fase binnen de slag) — duidelijk gelabeld zodat de gebruiker snapt waarom de absolute positie terugspringt.
   - Validatie: geheel getal, binnen ±(controllerbereik), negatieve relatieve dosering geweigerd (pomp doseert alleen vooruit); Naar positie mag terug (MOVETO), met bevestiging als het een terugbeweging is.
   - Exacte stappen naar de drive (geen afronding via f64-omwentelingen): p1 = doelstappen als i64/i32 binnen bereik; test dat 1, 51 199, 51 200, 51 201 en 25 600 exact zo in het CAN-frame komen.
2. **Kalibratie VSD** (pagina Kalibratie, profiel VSD): Camelot-methode (bestaande `Machine::calibrate`, `vsdcal.rs`): kies pomp(en), instellingen (fase 1 punten/runs/max°, fase 2 omwentelingen/runs, wachttijd; Camelot-standaard), wachtrij van pompen achter elkaar, voortgang (stap x/y, laatste gewicht), STOP. Weging op weegschaal van de eigen kop; kop zonder verbonden weegschaal → weigeren met melding.
3. **Kalibratie keramisch** (profiel Keramisch): kalibratie (omwentelingen, herhalingen, prime) en 360°-meetcurve via bestaande Engine-acties; vooruit-only en tellerbeveiliging blijven.
4. **Opslag in DB**: tabellen `calibration` (pomp, datum, soort, simulatie ja/nee, instellingen, samenvatting g/omw, g/slag, status actief/archief, opmerking) en `calibration_point` (fase/run/positie in stappen/gewicht/gerepareerd). Nieuwste geslaagde kalibratie = actief; oude blijven als historie. Eenmalige import van bestaande `machine/calibration/*.json` en keramische `results/characterization_*.json` (bestanden niet verwijderen). Afgebroken/mislukte kalibratie wordt opgeslagen als "afgebroken", nooit als actief.
5. **Resultaten**: grafiek fase 1 (gewicht vs positie, gerepareerde punten gemarkeerd), fase 2 (gewicht vs omwentelingen), keramisch curve; tabel met kerngetallen (g/slag, g/omw, spreiding tussen runs, aantal gerepareerde punten); historie per pomp met vergelijking actief vs vorige (afwijking in %). Kalibratiestatus op Pompen-pagina komt uit de DB (datum actieve kalibratie).
6. **Controle na kalibratie**: knop "Controle doseren" (x gram, standaard 1 g) met de actieve kalibratie (`Machine::dose_grams`), resultaat gevraagd/gedoseerd/afwijking % — opgeslagen bij de kalibratie.
7. Carry-over iteratie 1: profielwissel asynchroon (UI blokkeert niet; status "wisselen…"); Esc in "Kies"-menu geen STOP; LEESMIJ.md bijgewerkt naar nieuwe opzet; oude workflow-tests ruimen `%TEMP%\pump-rust-*` op.

## Iteratie 2 addendum (na Critic-review, FAIL)

- S1 Kalibratie-status per pomp, soort **en modus** (simulatie/hardware). Een simulatie-kalibratie archiveert of vervangt nooit een hardware-kalibratie. Doseren op hardware gebruikt alleen een hardware-kalibratie (anders weigeren met melding); simulatie mag een hardware-kalibratie lenen als er geen simulatie-kalibratie is. Pompen-pagina en historie tonen de modus.
- S2 Stappeninvoer is een tekstveld met eigen parser: spaties en punten als duizendtalscheiding ("25 600", "25.600" = 25 600); decimalen/komma in stappen geweigerd met melding; omw/graden accepteren Nederlandse komma ("0,5"). Geen sleep-invoer.
- S3 Splitsing doel naar drive gelijk aan Camelot (`VSDDispensePosition.cpp` modf + afkappen richting nul): p2 = doel / spr (richting nul), p3 = doel % spr (zelfde teken). Voorbeeld −12 800 → p1 −12 800, p2 0, p3 −12 800.
- Ook: v1→v2 positie-omrekening gebruikt de ingestelde stappen/omw; opdrachten tijdens "Profiel wisselen…" geweigerd; weegschaal-worker onderbreekt een lopende stabiele weging bij STOP; bevestiging terugbeweging vervalt als het doel verandert.
- Iteratie 2 PASS (Critic, 2 nov 2026, na 2 herstelrondes); 85 tests + 5 bridge-tests, 0 waarschuwingen; opgeleverd. Back-up v1-DB in `data/backup-v1/`.
- Carry-over: stop-vlag per kop blijft staan als binnen ~100 ms na STOP een nieuwe weging op dezelfde kop start (alleen wegen; volgende poging werkt).
- Planner-instructie bij oplevering: oude app (iteratie 1) eerst sluiten vóór de eerste start van iteratie 2 (schema-upgrade).

**Buiten scope**: pomptest-reeksen, Word-rapport (iteratie 3), recepten, productie, lift/transport.

**Acceptatie**
- K1 VSD-positie in stappen: invoer 25 600 st → exact p1 = start+25 600 (relatief) resp. 25 600 (absoluut) in het frame; eenheidsomrekening klopt; ongeldige invoer geweigerd met melding.
- K2 VSD-kalibratie in simulatie levert curve fase 1 + fase 2, opgeslagen in DB, zichtbaar in grafiek en historie; tweede kalibratie maakt de eerste archief.
- K3 STOP tijdens kalibratie: binnen 1 s gestopt, opgeslagen als afgebroken, actieve kalibratie onveranderd.
- K4 Kalibratie weigert zonder weegschaal van de eigen kop, of als pomp niet actief.
- K5 Keramische kalibratie en meetcurve in simulatie: opgeslagen in DB, grafiek, tellerbeveiliging intact (bestaande tests groen).
- K6 Import oude kalibratiebestanden één keer, idempotent.
- K7 Controle doseren in simulatie binnen ±2 % op gesimuleerde pomp; resultaat bij kalibratie opgeslagen.
- K8 Profielwissel blokkeert UI niet (> 100 ms frame-tijd niet toegestaan tijdens wissel).
- K9 `cargo test` twee keer groen, release-build 0 waarschuwingen, smoke-test uitgebreid met K1/K2/K3/K5 en groen.
# Iteratie 3 — Pomptest (reeks/willekeurig, correctie 0,1 %)

Doel: een pomp (VSD of keramisch) doseertesten laten doen zoals Thomas ze nu met de hand doet (0,5–20 g, ±1 %-criterium), resultaten bewaren en er direct een kort Word-verslag uit halen.

**Scope**
1. **Testopzet** (pagina Pomptest): pomp kiezen (VSD: elke actieve pomp, weging op eigen kop; keramisch: de pompbank). Modus:
   - **Eén gewicht** × n herhalingen;
   - **Reeks**: lijst gewichten (bv. 0,5; 1; 2; 3; 4; 5; 10; 15; 20) × n herhalingen per gewicht, volgorde oplopend of door elkaar;
   - **Willekeurig**: n doses tussen min en max g (gelijkmatig verdeeld, zaad vastgelegd zodat de test herhaalbaar is).
   Tolerantie instelbaar (standaard ±1 %), wachttijd voor stabiel gewicht, tarra per dosis aan/uit, pauze na elke dosis aan/uit.
2. **Uitvoeren**: dosis voor dosis met de actieve kalibratie in de juiste modus (`calibration_for_dosing`; hardware zonder hardware-kalibratie → weigeren). Live tabel (nr, gevraagd, gedoseerd, afwijking g en %, binnen tolerantie ja/nee, duur) en grafiek (afwijking % per dosis met tolerantieband). STOP binnen 1 s; gestopte test blijft bewaard als "gestopt" met de doses tot dan.
3. **Correctie in 0,1 %-stappen**: tijdens een test (tussen doses) en daarbuiten aan te passen, −20 … +20 %; geldt vanaf de volgende dosis en wordt per dosis vastgelegd. Voorstel "aanbevolen correctie" = −gemiddelde afwijking % van de laatste n doses, afgerond op 0,1 %, alleen met een klik over te nemen. Keramisch: bestaande correctie en tellerbeveiliging blijven.
4. **Pomp/instellingen tijdens test**: pompinstellingen (snelheid/acceleratie, keramisch type/stappen) aanpasbaar tussen doses zonder opnieuw verbinden; wijziging wordt bij de dosis vastgelegd.
5. **Uitslag per gewicht**: gemiddelde afwijking (% en g), spreiding (st.dev. %), kleinste/grootste afwijking, aantal binnen tolerantie (x/n, %), g/min doseersnelheid. Uitschieters worden **niet** automatisch verwijderd; de gebruiker kan een dosis handmatig uitsluiten met een verplichte reden — uitgesloten doses blijven zichtbaar (doorgestreept) en in het verslag staat hoeveel er met welke reden zijn uitgesloten.
6. **Opslag**: DB-tabellen `test_run` (pomp, modus sim/hw, kalibratie-id, instellingen, tolerantie, status, start/eind, opmerking) en `test_dose` (nr, gevraagd, gedoseerd, correctie %, pompinstellingen-snapshot, duur, uitgesloten + reden). Eenmalige import van oude `results/dosering_*.json` (bestanden blijven staan). Historie per pomp (lijst testen, openen, vergelijken).
7. **Word-verslag** (bestaande `report.rs` aanpassen): kort directieverslag in Thomas' stijl (NL, kort, eigen oordeel, weinig detail): inleiding (pomp, datum, opzet), tabel per gewicht (gevraagd, gem. afwijking, spreiding, kleinste/grootste afwijking, binnen ±1 %), grafiek, conclusie/advies als in te vullen plek (geen verzonnen conclusie), bijlage met alle doses. Ook voor meerdere testen van dezelfde pomp samen (selectie in historie). Simulatie-testen krijgen duidelijk "SIMULATIE" in titel.
8. **Export CSV** van een test (doses + samenvatting), Nederlandse notatie (komma decimaal, puntkomma-scheiding) zodat Excel het direct opent.
9. **Druppelgrootte** (keramisch, was eerder gevraagd): bij kleine doses de spreiding tegen gevraagd gewicht tonen als indicatie van de kleinste zinvolle dosis; geen extra hardware.
10. Carry-over: stop-vlag per kop venster (zie addendum iteratie 2) — reset bij start opdracht als de vorige weging al gestopt is.

**Buiten scope**: recepten, productie, analyse over meerdere pompen (iteratie 6).

## Iteratie 3 addendum (na Critic-review, FAIL)

- U1 Aanbevolen correctie houdt rekening met de correctie per dosis: per getelde dosis c_i' = ((1+c_i/100)/(1+e_i/100) − 1)·100, gemiddeld over de laatste n getelde doses, afgerond 0,1 %, begrensd ±20 %. UI toont over welke doses het voorstel gaat.
- Timingtests krijgen ruime grenzen (geen flakiness onder parallelle belasting).
- Iteratie 3 PASS (Critic, 2 nov 2026, na 1 herstelronde); 101 tests, 0 waarschuwingen; opgeleverd. Back-up DB vóór v3 in `data/backup-v2/`.
- Carry-over naar iteratie 4: doses met 0 g of negatief gewicht overslaan in het correctievoorstel (met hint), i.p.v. hele voorstel blokkeren of −20 %.
- Planner-besluit P7: de database is de bron voor doseertesten; de Engine schrijft bij testen vanuit Pomptest geen losse `dosering_*.json` meer.

**Acceptatie**
- T1 Reeks 0,5/1/5 g × 3 in simulatie: 9 doses, uitslag per gewicht klopt (rekenwerk getest met vaste getallen), opgeslagen.
- T2 Willekeurig met vast zaad geeft twee keer exact dezelfde gewichten.
- T3 Correctie 0,1 %-stap tussen doses: volgende dosis gebruikt hem (test op doelstappen/gevraagde gram), vastgelegd per dosis; aanbevolen correctie juist berekend.
- T4 STOP tijdens test < 1 s, status gestopt, doses tot dan bewaard.
- T5 Hardware zonder hardware-kalibratie geweigerd; simulatie-test nooit verward met hardware (verslag en historie).
- T6 Uitsluiten vraagt reden, uitslag herberekend, verslag noemt uitsluiting; niets stil weggelaten.
- T7 Word-verslag opent (geldig docx, getest op structuur), bevat tabel + grafiek + bijlage; CSV opent met juiste getallen.
- T8 Import oude doseerbestanden één keer.
- T9 Instelling wijzigen tussen doses zonder opnieuw verbinden; vastgelegd.
- T10 `cargo test` twee keer groen, 0 waarschuwingen, smoke-test met T1/T3/T4.
# Iteratie 4 — Ingrediënten + Recepten (stappen sequentieel/parallel, LH1/LH2) + uitvoeren op één pot

Thomas (2 nov 2026): "Ga hierna gewoon verder met de andere iteraties. app moet compleet af." → Iteraties 4–6 doorlopen zonder tussentijdse goedkeuring; open beslissingen B3–B5 neemt de Planner (zie Planner-besluiten P8–P10).

**Scope**
1. **Ingrediënten** (pagina): lijst per profiel; naam (uniek), dichtheid (optioneel), opmerking, gekoppelde pomp(en) met kop. Een ingrediënt mag op LH1 én LH2 een pomp hebben (dan kiest het recept per stap de kop, standaard de kop met een actieve, gekalibreerde pomp; bij beide: instelbare voorkeur). Waarschuwingen: ingrediënt zonder pomp, pomp zonder ingrediënt, pomp niet actief/niet gekalibreerd (in de modus van de verbinding). Verwijderen alleen als geen recept het gebruikt (anders melding welke). Import/export CSV (NL-notatie).
2. **Recepten** (pagina): lijst + editor. Recept = naam (uniek), omschrijving, potmaat/potgewicht (optioneel), geordende stappen; stap = één ingrediënt (sequentieel) of groep (parallel) met gram per ingrediënt; stappen omhoog/omlaag, kopiëren, dupliceren; totaalgewicht; per stap en per kop zichtbaar welke kop doseert (uit ingrediënt→pomp). Validatie volgens Receptmodel (bindend). Versiebeheer: wijzigen van een recept dat al gebruikt is maakt een nieuwe versie (oude productieruns verwijzen naar hun versie).
3. **Tolerantie per recept/ingrediënt** (standaard ±1 %), en per stap de keuze "bijdoseren bij te weinig" (aan/uit, max 1 keer, alleen als ondergrens niet gehaald; te veel wordt gemeld, nooit gecorrigeerd).
4. **Uitvoeren op één pot** (handmatig, zonder transport): kop(pen) kiezen die het recept vraagt; per kop: lift omhoog (PLC, alleen na expliciete klik "Start pot"), tarra, stappen in volgorde — sequentieel = één pomp, parallel = alle pompen van die stap op die kop tegelijk starten, wachten tot klaar, wegen; stap met items op beide koppen loopt per kop tegelijk; gewichtstoewijzing volgens Receptmodel (parallel per kop naar verhouding, gemarkeerd). Lift omlaag aan het eind en bij STOP/fout. Resultaat per ingrediënt: gevraagd, gedoseerd, afwijking, binnen tolerantie, bijgedoseerd ja/nee.
   - Recept met ingrediënten op beide koppen op één pot (handmatig): eerst LH1-deel, dan melding "zet pot onder LH2" (operator), dan LH2-deel. In productie (iteratie 5) gaat dat via transport.
   - Doseren met `calibration_for_dosing` per pomp; parallel doseren op één kop vraagt één gecombineerd CAN-commando-blok (alle drives starten, daarna wachten op alle antwoorden), niet na elkaar.
5. **Opslag**: tabellen `ingredient` (uitbreiden), `ingredient_pump` (ingredient, pomp, voorkeur), `recipe`, `recipe_version`, `recipe_step`, `recipe_item`, `pot_run` (recept-versie, modus, status, start/eind, barcode NULL, order NULL) en `pot_item` (stap, ingrediënt, pomp, kop, gevraagd, gedoseerd, toegewezen-parallel ja/nee, bijgedoseerd, afwijking, status). Elke stap direct opgeslagen.
6. **Simulatie**: SimWorld per kop met PLC-emulator voor lift; alles doorloopbaar zonder machine.

**Acceptatie**
- R1 Recept met stappen: seq A (LH1), parallel {B,C} (LH1), parallel {D (LH1), E (LH2)}, seq F (LH2) → validatie OK, uitvoering in simulatie in juiste volgorde/kop, B en C gelijktijdig gestart (tijdstempels), resultaten opgeslagen.
- R2 Validatie weigert: ingrediënt zonder pomp, inactieve/ongekalibreerde pomp (per modus), zelfde ingrediënt twee keer in één parallelle stap, 0 of negatief gram, leeg recept — met melding welke.
- R3 Parallelle toewijzing naar verhouding klopt (vaste getallen) en is gemarkeerd.
- R4 Te weinig → bijdoseren 1× als aan; te veel → gemeld, niet gecorrigeerd; beide in resultaat.
- R5 STOP tijdens recept < 1 s: pompen gestopt, lift omlaag (emulator), pot-run "gestopt" met wat er gedoseerd is.
- R6 Recept wijzigen na gebruik → nieuwe versie; oude run toont oude versie.
- R7 Ingrediënt verwijderen dat in gebruik is → geweigerd met recepten-lijst.
- R8 `cargo test` 2× groen, 0 waarschuwingen, smoke uitgebreid met R1/R5.

## Iteratie 4 addendum (na Critic-review, FAIL)

- L1 Lift omhoog stuurt P1 = hoogte (cm) × 10, zoals Camelot `Lift.cpp`. Hoogte: standaard per kop in Instellingen, te overschrijven per recept (pothoogte). Op hardware zonder hoogte > 0 → niet starten, melding.
- L2 Geaccepteerde PLC-antwoorden gelijk aan Camelot: lift Start 2/4, lift Stop 9 (5 alleen voor emulator), unit Start 2, unit Stop 9. Antwoord 16 (geen pot) of onverwacht → kop mislukt: niet doseren, lift omlaag, pot "fout" met reden.
- L3 (vervroegd uit iteratie 5) Koppen onafhankelijk: elke kop eigen ritme en eigen CAN-blok; een trage kop of een drivefout op de ene kop raakt de andere kop/pot niet.
- Iteratie 4 PASS (Critic, 2 nov 2026, na 1 herstelronde); 121 tests, 0 waarschuwingen; opgeleverd. Back-up DB vóór v4 in `data/backup-v3/`.
- Verplicht in iteratie 5 (uit Critic-minors): paniek in een kop- of pot-thread → lift van die kop altijd omlaag (guard/catch_unwind), status opgeruimd, juiste kop gemeld; PLC-antwoord alleen accepteren als het bij de verzonden opdracht hoort (snd-teller); `check_height` ook bij start van een pot.

# Iteratie 5 — Machine/lijn + scanner + orders/barcodes/labels + Productie met Start-cyclus + productiehistorie

**Scope**: zie bindend stuk "Productieproces" hierboven, plus:
1. **Machine/lijn** (pagina, handbediening): lijn 1310 start/stop, transport één plek, liften LH1/LH2 omhoog/omlaag, units, barcode-proces, scanner-test; alles alleen na expliciete klik; vrije PLC-opdrachten op echte PLC nu toegestaan op deze pagina met bevestiging (P5 vervalt hier, blijft op Monitors).
2. **Scanner**: generiek protocol (COM via bridge of TCP direct), instellingen per profiel, test-knop, simulatie (scanner geeft de code van de pot die het lijnmodel op de scanplek heeft).
3. **Orders/potten/barcodes**: order = recept-versie × aantal potten (+ naam/batch/prioriteit); potten krijgen barcode = 10 cijfers pot-id; labelvel printen (P8); handmatig code invoeren/scannen om te koppelen.
4. **Productie-pagina**: wachtrij (volgorde/prioriteit aanpasbaar), Start / Pauze / Stop / Afbreken / Herstart na fout, lijnbeeld (scanplek, wachtplek, LH1, LH2, uit) met pot/order/status, voortgang per pot, meldingen. Cyclus exact volgens "Productieproces": voorcontrole, lijn start, doseren beide koppen tegelijk, scannen, transport alleen met beide liften omlaag, leeglopen, foutafhandeling met Hold/Herstart, time-outs op elke PLC-wachtstap, lijnbeeld in DB (herstel na herstart app).
5. **PLC-initialisatie** (P9).
6. **Productiehistorie**: per pot (barcode, order, recept-versie, per ingrediënt resultaat), filter op datum/order/recept/status.
7. Carry-over: oude machine/PLC-paden in `workflow::Engine` opruimen; `vsd_bridge.py wait()` stop-bewust; `Devices::status()` niet per aanroep kopiëren.

**Acceptatie**
- P1 Simulatie: 3 orders (totaal 7 potten, recepten met LH1-, LH2- en gemengde delen) → Start → lijn draait zelf leeg, elke pot op beide koppen correct gedoseerd, LH1 en LH2 tegelijk bezig op verschillende potten (tijdstempels), transport alleen met liften omlaag, lijn 1310 Start aan begin en Stop aan eind.
- P2 No-read, onbekende code, geen pot (16) → juiste afhandeling volgens plan, gelogd en in historie.
- P3 Pauze/Stop/Afbreken/Fout-Herstart gedragen zich volgens plan; afbreken → liften omlaag, potten blijven bekend als "afgebroken".
- P4 PLC antwoordt niet → time-out → Hold met melding, geen vastloper; Herstart hervat.
- P5 App herstart midden in productie → lijnbeeld hersteld, geen pot dubbel gedoseerd (geen automatische herstart; operator beslist).
- P6 Barcode-uniekheid; gebruikte code nooit opnieuw gedoseerd.
- P7 Labelvel opent en bevat juiste codes (Code 128, controleerbaar door decoderen in test).
- P8 `cargo test` 2× groen, 0 waarschuwingen, smoke met P1/P3.

## Iteratie 5 addendum (na Critic-review, FAIL)

- M1 Echte gelijktijdigheid: CAN-worker blokkeert niet per doseerblok. Blok sturen, wachtende blokken bijhouden, kort pollen per drive, per blok antwoord + SETPOS zodra al zijn drives klaar zijn; blok van de andere kop wordt direct gestuurd. Drivefout raakt alleen de eigen kop. Bewijs: doseertijden LH1/LH2 overlappen in P1.
- M2 Liftstand altijd bekend vóór transport: bij Start, Herstart en "Doorgaan na herstel" beide liften Stop sturen en wachten op 9; `production_check` weigert als een lift volgens de PLC omhoog staat.
- M3 Transport-antwoord 16 = transport niet gebeurd: geen Stop, lijnmodel niet opschuiven; staan er volgens het model potten op de lijn → Hold met melding.
- M4 Barcodeproces: wachten op scanner en PLC tegelijk; bij 16 scan direct afbreken.
- Iteratie 5 PASS (Critic, 3 nov 2026, na 4 herstelrondes: gelijktijdig doseren, liftstand, STOP op bridge-pad, flaky STOP/DB-open, unit-Stop-wacht bij normaal einde); 151 tests 3× groen bij Planner, 0 waarschuwingen; opgeleverd. Back-up DB vóór v5 in `data/backup-v4/`.
- Carry-over naar iteratie 6: Herstart na een unit-Stop-Hold herhaalt unit Stop van die kop en wacht op 9 vóór transport.
- Op de machine te controleren: PLC-belasting bij poll 15 ms; laat unit-antwoord na STOP; lifthoogte, scanner-type/poort.

# Iteratie 6 — Analyse + export CSV/Excel

**Scope**
1. **Analyse-pagina**: historie per pomp (kalibraties: g/slag-trend; testen: afwijking per gewicht door de tijd), per ingrediënt (productieafwijking), per recept (potten, % binnen tolerantie), filters op periode/kop/modus (simulatie standaard uit).
2. **Export**: CSV (NL) en **Excel .xlsx** (eigen minimale writer, geen zware dependency; meerdere tabbladen: potten, ingrediënt-resultaten, testen, kalibraties) van de gefilterde selectie.
3. **Dashboard-kaart** op Analyse: vandaag/week aantal potten, % binnen tolerantie, slechtste 5 ingrediënten/pompen.
4. Opruimen: logtabel-retentie (instelbaar, standaard 90 dagen), LEESMIJ compleet, alle "Volgt in iteratie"-kaarten weg.

**Acceptatie**
- N1 Cijfers op Analyse kloppen met handberekening op vaste testdata.
- N2 .xlsx opent (geldige OOXML, getest op structuur), tabbladen en getallen kloppen; CSV NL.
- N3 Simulatie standaard uit de analyse; aan te zetten, dan duidelijk gemarkeerd.
- N4 Geen pagina meer met "Volgt in iteratie".
- N5 `cargo test` 2× groen, 0 waarschuwingen, smoke met analyse/export.

## Iteratie 6 — status

- PASS (Critic, 3 nov 2026, incl. eindcontrole hele app); daarna 3 minors (xlsx-celgrens, cellStyles, DST-middernacht, slechtste-lijst alleen gewogen). Planner: 161 tests 3× groen, 0 waarschuwingen, smoke + bridge OK; opgeleverd. Back-up DB vóór v6 in `data/backup-v5/`.
- Na oplevering (3 nov 2026), op basis van echte CAN-sporen bord 626: bridge neemt het eerste geldige antwoord na verzenden (done gevolgd door foutset = geslaagd, zoals Camelot), foutsets apart gelogd; STATUS accepteert update (1). Critic PASS; Planner 3× groen; opgeleverd. Op machine controleren: foutset ook na SETPOS?
- App functioneel compleet volgens dit plan. Niet op echte hardware getest: CAN-pompen (bus was stil), lift/transport/scanner in productie, VSD-kalibratie.

## Planner-besluiten iteratie 4–6

- P8 (B3) Barcodes: Pump Studio maakt de codes; labels als printbaar labelvel (HTML met Code 128 als SVG, opent in standaardbrowser → printen) plus optioneel ZPL rechtstreeks naar een netwerkprinter (IP:9100) als die in Instellingen staat. Handmatig code invoeren blijft mogelijk.
- P9 (B4) PLC-opstartreeks (Abort/PowerUp/Reset) nooit automatisch; alleen via knop "PLC initialiseren (zoals Camelot)" op Machine/lijn met bevestiging en waarschuwing dat Camelot niet mag draaien.
- P10 (B5) Start is een knop in de app; geen fysieke startknop (Camelot heeft er geen code voor).
- P11 Commits: geen akkoord (B1) → niet committen.
# Iteratie 5 — Machine/lijn + scanner + orders/barcodes/labels + Productie met Start-cyclus + productiehistorie
# Iteratie 6 — Analyse + export CSV/Excel

(Scope en acceptatie per iteratie worden uitgewerkt bij de start van die iteratie, op basis van de feedback van de vorige.)

## Planner-besluiten

- P1 Simulatie blijft in elke functie (testbaarheid, zie kwaliteitslat 7).
- P2 Hardwarecode wordt hergebruikt; UI wordt opnieuw opgezet.
- P3 Word-rapport, tellerbeveiliging (keramisch) en druppelanalyse komen terug in de iteratie waar ze horen (2/3), niet in iteratie 1.
- P4 Camelot-database wordt niet gebruikt (alleen eenmalig geraadpleegd voor bedrading en adressen).

## Open beslissingen voor Thomas

- B1 Mag elke geslaagde iteratie als git-commit worden vastgelegd? (aanbevolen: ja, één commit per iteratie)
- B2 Oude app: vervangen door de nieuwe zodra iteratie 1 klaar is, of oude exe bewaren tot iteratie 5? (aanbevolen: oude exe als `pump-studio-oud.exe` bewaren)

- B3 Barcodes: labels printen vanuit Pump Studio (welke printer?) of voorgedrukte/handmatige codes koppelen aan potten? (aanbevolen: Pump Studio maakt codes en print labels; ook handmatig code invoeren mogelijk)
- B4 Mag Pump Studio bij verbinden de opstartreeks van Camelot (Abort/PowerUp/Reset) naar de PLC sturen? Alleen relevant als Camelot niet meer draait.
- B5 Fysieke startknop op de machine: bestaat die? Camelot heeft er geen code voor; Start is in iteratie 5 een knop in de app.

## Carry-over

- 26 compilerwaarschuwingen `float_literal_f32_fallback` (Rust 1.99) opruimen in iteratie 1.
