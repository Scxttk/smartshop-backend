#!/usr/bin/env python3
"""Wörterbuch-Entwurf: taggt aktuelle Angebote regelbasiert mit Alltagsbegriffen."""
import sqlite3, re, os, json
from collections import Counter, defaultdict

DB = os.path.expanduser("~/.local/share/smartshop/smartshop.db")

# Kategorien, die klar Non-Food sind (Ketten-Marketing-Kategorien)
NONFOOD_CAT = re.compile(r"mode|style|heim|haus|garten|haustier|tierbedarf|tiernahrung|pflanzen|angeln|elektro|medien|kinderzimmer|wäschepflege|schulstart|kochen-und-grillen|drogerie|spielzeug|alltagshelfer|technik|spielwaren|baumarkt|multimedia|bekleidung|schuhe|camping|auto|buero|non.?food", re.I)

# Wörterbuch: begriff -> (exakte tokens, komposita-suffixe, blockliste)
V = {
 "brot":(["brot","broetchen","brötchen","toast","baguette","ciabatta"],["brot","broetchen","brötchen","toast"],["brotaufstrich","aufbackbrötchen?"]),
 "milch":(["milch","frischmilch","vollmilch","buttermilch","mandeldrink","haferdrink","sojadrink"],["milch"],["milchreis","milchschnitte","milchbrötchen","kokosmilch","milcheis","milchschokolade","kondensmilch","sonnenmilch","kokosnussmilch","milka","knoppers","milch schnitte"]),
 "butter":(["butter","markenbutter"],[],["butterkäse","buttergemüse","erdnussbutter","buttermilch","butterkeks"]),
 "käse":(["käse","kaese","käsescheiben","käsesnack","cottage","gouda","emmentaler","edamer","maasdamer","bergkäse","butterkäse","cheddar","parmesan","grana","halloumi"],["käse","kaese"],["käsekuchen","frischkäse","croissant","leberkäse"]),
 "frischkäse":(["frischkäse","frischkaese"],[],[]),
 "mozzarella":(["mozzarella"],["mozzarella"],[]),
 "feta":(["feta","hirtenkäse","schafskäse"],[],[]),
 "quark":(["quark","speisequark"],["quark"],["quarkbällchen"]),
 "joghurt":(["joghurt","jogurt"],["joghurt","jogurt","ghurt"],[]),
 "sahne":(["sahne","schlagsahne","schmand","creme fraiche","crème fraîche"],["sahne"],["sahnetorte","sahnebonbon"]),
 "eier":(["eier","ei","freilandeier","bio-eier"],["eier"],["eierlikör","eiernudeln","eierkuchen"]),
 "tomaten":(["tomate","tomaten","rispentomaten","cherrytomaten","kirschtomaten","strauchtomaten","romatomaten","cocktailtomaten"],["tomaten"],["tomatenmark","tomatensoße","tomatensauce","tomatenketchup","tomatensaft","tomatensuppe"]),
 "gurke":(["gurke","gurken","salatgurke","salatgurken","minigurken"],["gurke","gurken"],["gewürzgurken","essiggurken","gurkensticks"]),
 "paprika":(["paprika","spitzpaprika"],["paprika"],["paprikachips","paprikasauce"]),
 "salat":(["salat","eisbergsalat","kopfsalat","feldsalat","rucola","blattsalat","salatherzen"],["salat"],["salatdressing","salatsoße","nudelsalat","kartoffelsalat","krautsalat","fleischsalat","thunfisch-salat","salatcreme","salatmayonnaise"]),
 "zwiebeln":(["zwiebel","zwiebeln","speisezwiebeln","gemüsezwiebeln","rote zwiebeln"],["zwiebeln"],["röstzwiebeln","zwiebelringe","zwiebelkuchen","zwiebelmettwurst"]),
 "knoblauch":(["knoblauch"],[],["knoblauchbaguette","knoblauchsauce"]),
 "kartoffeln":(["kartoffel","kartoffeln","speisekartoffeln","frühkartoffeln"],["kartoffeln"],["kartoffelsalat","kartoffelchips","kartoffelknödel","kartoffelpuffer","süßkartoffeln","kartoffelecken"]),
 "möhren":(["möhre","möhren","karotten","moehren","bundmöhren"],["möhren"],[]),
 "äpfel":(["apfel","äpfel","aepfel"],["äpfel"],["apfelsaft","apfelmus","apfelschorle","apfelkuchen","apfelessig","apfelringe"]),
 "bananen":(["banane","bananen"],[],["bananenmilch"]),
 "zitronen":(["zitrone","zitronen","limetten"],[],["zitronensaft","zitronenlimonade"]),
 "orangen":(["orange","orangen","mandarinen","clementinen"],[],["orangensaft","orangenlimonade"]),
 "beeren":(["erdbeeren","himbeeren","blaubeeren","heidelbeeren","brombeeren","johannisbeeren","beerenmix"],["beeren"],["erdbeermarmelade","erdbeerjoghurt"]),
 "trauben":(["trauben","tafeltrauben","weintrauben"],["trauben"],["traubensaft","traubenzucker"]),
 "melone":(["melone","wassermelone","honigmelone","galiamelone","cantaloupe"],["melone"],[]),
 "pfirsich":(["pfirsich","pfirsiche","nektarinen","aprikosen","flachpfirsiche","kirschen","pflaumen","plattnektarinen"],["pfirsiche","aprikosen","nektarinen","pflaumen"],[]),
 "avocado":(["avocado","avocados"],[],[]),
 "zucchini":(["zucchini"],[],[]),
 "aubergine":(["aubergine","auberginen"],[],[]),
 "brokkoli":(["brokkoli","broccoli","blumenkohl","kohlrabi","chicorée","chicoree"],[],[]),
 "pilze":(["champignon","champignons","pilze","pfifferlinge"],["pilze","champignons"],["pilzpfanne","pilzsauce"]),
 "hackfleisch":(["hackfleisch","hack","gehacktes","rinderhack","gemischtes hack"],["hackfleisch","hack"],["hacksteaks"]),
 "hähnchen":(["hähnchen","haehnchen","hähnchenbrust","hähnchenbrustfilet","hähnchenschenkel","hähnchenflügel","poulet","chicken","wings"],["hähnchen","medaillons"],["schweinemedaillons"]),
 "pute":(["pute","putenbrust","putenschnitzel","putensteaks"],["pute"],["putenwurst"]),
 "kokosmilch":(["kokosmilch","kokosnussmilch"],[],[]),
 "lamm":(["lamm","lammfilets","lammlachs","lammkeule"],[],[]),
 "schwein":(["schwein","schweine","schweinemedaillons","kasseler","schweinefilet","schweineschnitzel","schweinebraten","schweinesteaks","nackensteaks","schweinelachs","kotelett","krustenbauch"],["kotelett","nuggets"],[]),
 "rind":(["rindersteak","rinderfilet","rinderbraten","rumpsteak","entrecote","rinderrouladen","rinder","beinscheiben","roastbeef","gulasch","corned beef","hüftsteaks","patties"],["steak","steaks"],[]),
 "bratwurst":(["bratwurst","rostbratwurst","grillwurst","bratwürste"],["bratwurst","bratwürste"],[]),
 "wurst":(["wurst","salami","schinken","mortadella","lyoner","leberwurst","mettwurst","wiener","würstchen","aufschnitt","mett","edelsalami","cabanossi","chipolata","sülze","serrano","schinkenwürfel","currywurst","currykrakauer","leberkäse"],["wurst","würstchen","schinken","salami","aufschnitt"],[]),
 "fisch":(["lachs","lachsfilet","forelle","kabeljau","seelachs","garnelen","shrimps","fischstäbchen","matjes","hering","thunfisch"],["fisch","filet"],["fischsauce","schwein","schweine","lamm","lammfilets","kasseler"]),
 "nudeln":(["nudeln","spaghetti","penne","fusilli","tagliatelle","tortellini","cappelletti","gnocchi","pasta","lasagne","ramen","ramyun"],["nudeln"],["nudelsalat","nudelsuppe"]),
 "reis":(["reis","basmati","basmatireis","langkornreis","jasminreis","risottoreis"],["reis"],["milchreis","reiswaffeln","reisdrink"]),
 "mehl":(["mehl","weizenmehl","dinkelmehl","panko","tempura","paniermehl"],["mehl"],[]),
 "zucker":(["zucker","rohrzucker","puderzucker"],["zucker"],["traubenzucker","vanillezucker","zuckerrübensirup"]),
 "salz":(["salz","meersalz","speisesalz"],[],["salzstangen","salzbrezeln"]),
 "öl":(["öl","olivenöl","rapsöl","sonnenblumenöl","speiseöl","erdnussöl","sesamöl","kokosöl"],["öl","oel"],[]),
 "essig":(["essig","balsamico"],["essig"],["essiggurken"]),
 "müsli":(["müsli","muesli","haferflocken","granola","cornflakes","cerealien"],["müsli","flocken"],["müsliriegel"]),
 "marmelade":(["marmelade","konfitüre","fruchtaufstrich","brotaufstrich","honig","nutella","nussnougatcreme"],["marmelade","konfitüre"],[]),
 "kaffee":(["kaffee","espresso","coffee","kaffeebohnen","filterkaffee","kaffeepads","kaffeekapseln"],["kaffee"],["kaffeesahne","eiskaffee","kaffeeweißer"]),
 "tee":(["tee","kräutertee","früchtetee","grüner tee","schwarztee","matcha","ländertee"],["tee"],["eistee"]),
 "wasser":(["wasser","mineralwasser","sprudel"],["wasser"],[]),
 "saft":(["saft","orangensaft","apfelsaft","multivitaminsaft","nektar","schorle"],["saft","schorle"],[]),
 "limonade":(["limonade","cola","coca-cola","fanta","sprite","mezzo mix","limo","eistee","energy drink","energydrink"],["limonade"],[]),
 "bier":(["bier","pils","pilsener","radler","weißbier","weizen","helles","dunkel","schwarzbier","biermischgetränk"],["bier"],["bierschinken","trauben","tafeltrauben"]),
 "wein":(["wein","rotwein","weißwein","rosé","sekt","prosecco","secco","fruchtsecco","chardonnay","merlot","riesling","grauburgunder","sauvignon","blanc"],["wein"],["weinsauerkraut","weintrauben","weinessig"]),
 "schokolade":(["schokolade","tafelschokolade","pralinen","schokoriegel"],["schokolade"],["schokoladenpudding","trinkschokolade"]),
 "kekse":(["kekse","butterkeks","cookies","gebäck","waffeln"],["kekse","keks"],[]),
 "chips":(["chips","tortilla","nachos","erdnussflips","flips","cracker","salzstangen","kartoffelringe"],["chips"],["kartoffelchips fällt unter chips"]),
 "eis":(["eis","eiscreme","speiseeis","eistafel","eiskonfekt","waffelhörnchen","eisbecher"],["eis"],["eistee","eiswürfel","eiskaffee"]),
 "pizza":(["pizza","steinofenpizza"],["pizza"],["pizzabrötchen","pizzakäse"]),
 "tiefkühlgemüse":(["tiefkühlgemüse","rahmspinat","spinat","erbsen","gemüsemix","kaidergemüse"],["gemüse"],["buttergemüse zulässig"]),
 "pommes":(["pommes","pommes frites","wedges","kroketten","rösti"],[],[]),
 "tofu":(["tofu","vegane","vegan","veggie","fleischersatz","falafel","gemüsebällchen"],[],[]),
 "eintopf":(["eintopf","suppe","brühe","bouillon"],["eintopf","suppe"],[]),
 "konserven":(["mais","kidneybohnen","kichererbsen","linsen","bohnen","tomatenmark","passierte tomaten","gehackte tomaten","sauerkraut","rotkohl","oliven","pfefferoni","brechbohnen","datteln"],[],[]),
 "soßen":(["ketchup","mayonnaise","mayo","senf","grillsauce","bbq sauce","sriracha","sojasauce","dressing","pesto"],["sauce","soße","sosse"],[]),
 "gewürze":(["pfeffer","paprikapulver","curry","gewürz","gewürze","gewürzmischung","kräuter","koriander","ingwer"],["gewürz"],["gewürzgurken"]),
 "backwaren":(["croissant","kuchen","torte","berliner","muffins","brezel","laugengebäck","hefezopf","stollen","backmischung","weckli","flammkuchenböden","törtchen"],["kuchen","backmischung","törtchen"],[]),
 "windeln/hygiene":(["windeln","toilettenpapier","küchenrolle","taschentücher","zahnpasta","duschgel","shampoo","deo","deodorant","waschmittel","spülmittel","vanish","lenor","zewa"],["papier","waschmittel"],["stofftaschentücher"]),
 "spirituosen":(["vodka","wodka","whisky","whiskey","gin","rum","likör","likoer","korn","tequila","aperol","batida","asti","spirituose","jack daniels","jim beam","bittergetränke","doppelkorn","edelbrand","wermut"],["likör","limes"],[]),
 "pudding":(["pudding","dessert","götterspeise","grießpudding","mousse","milchreis"],["pudding"],[]),
 "nüsse":(["nüsse","erdnüsse","cashewkerne","cashew","erdnuss","mandeln","pistazien","pistazienkerne","walnüsse","studentenfutter","trockenfrüchte"],["kerne","nüsse"],[]),
 "margarine":(["margarine","rama","cremefine","pflanzencreme"],["margarine"],[]),
 "fertiggericht":(["fertiggericht","fertiggerichte","tortelloni","maultaschen","bowl","ravioli","mikrowellengericht","instant","gyoza","onigiri","wrap","wraps"],["gericht"],[]),
 "knäckebrot":(["knäckebrot","knusperbrot","zwieback","wasa","reiswaffeln"],[],[]),
 "schoten/hülsen":(["kaiserschoten","zuckerschoten","edamame","bohnen grün"],["schoten"],[]),
 "protein/fitness":(["proteinriegel","high protein","proteindrink","proteinpulver","whey","trinkmahlzeiten","trinkmahlzeit"],[],[]),
}

# Marke → Kategorie (Fallback, wenn Wörterbuch nichts trifft). "NONFOOD" = aussortieren.
MARKEN = {
 # Bier
 "bitburger":"bier","beck's":"bier","becks":"bier","radeberger":"bier","corona":"bier","peroni":"bier",
 "krombacher":"bier","sternburg":"bier","schöfferhofer":"bier","warsteiner":"bier","paulaner":"bier",
 "erdinger":"bier","franziskaner":"bier","eibauer":"bier","ur-krostitzer":"bier","wernesgrüner":"bier",
 "freiberger":"bier","5,0 original":"bier","heineken":"bier","desperados":"bier","astra":"bier","lausitzer":"bier",
 # Getränke
 "red bull":"limonade","monster":"limonade","capri-sun":"limonade","adelholzener":"wasser","volvic":"wasser",
 "gerolsteiner":"wasser","vio ":"wasser","fritz-kola":"limonade","valensina":"saft","pfanner":"saft",
 "granini":"saft","hohes c":"saft","marathon":"limonade","yfood":"limonade",
 # Kaffee
 "nescafé":"kaffee","nescaf":"kaffee","jacobs":"kaffee","dallmayr":"kaffee","melitta":"kaffee",
 "l'or":"kaffee","lavazza":"kaffee","tchibo":"kaffee","magico":"kaffee",
 # Süßes & Snacks
 "milka":"schokolade","ferrero":"schokolade","katjes":"schokolade","haribo":"schokolade","lindt":"schokolade",
 "ritter sport":"schokolade","kitkat":"schokolade","nesquik":"schokolade","smarties":"schokolade","lion":"schokolade",
 "merci":"schokolade","toffifee":"schokolade","wrigley":"schokolade","bahlsen":"kekse","leibniz":"kekse",
 "brandt":"knäckebrot","coppenrath":"kekse","lambertz":"kekse","oreo":"kekse","lorenz":"chips",
 "funny-frisch":"chips","pringles":"chips","chio":"chips","pombär":"chips",
 # Molkerei
 "ehrmann":"joghurt","müller":"joghurt","danone":"joghurt","fruchtzwerge":"joghurt","landliebe":"joghurt",
 "weihenstephan":"milch","bauer":"joghurt","meggle":"butter","hochland":"käse","st. mang":"käse",
 "patros":"käse","grünländer":"käse","loose":"käse","cheestrings":"käse","lindenhof":"käse","adler":"käse","ergüllü":"frischkäse","miree":"frischkäse","kærgården":"butter","kaergarden":"butter","kerrygold":"butter","milprima":"joghurt","kids world":"joghurt","fruchtigurt":"joghurt","kuchenmeister":"kekse","borggreve":"kekse","oma hartmanns":"kekse","st. michel":"kekse","dickmann":"schokolade","storck":"schokolade","mentos":"schokolade","chupa chups":"schokolade","nimm2":"schokolade","halloren":"schokolade","milchmäuse":"schokolade","milka":"schokolade","knoppers":"schokolade","milch schnitte":"schokolade","suchard":"kakao","fuze tea":"limonade","active o2":"limonade","orangina":"limonade","vitamalz":"limonade","capri sun":"limonade","voelkel":"saft","lübzer":"bier","spaten":"bier","benediktiner":"bier","carlsberg":"bier","anheuser":"bier","bud ":"bier","kloster scheyern":"bier","gerstacker":"wein","frizzade":"wein","secconade":"wein","cavino":"wein","cecchi":"wein","lenz moser":"wein","doppio passo":"wein","calvet":"wein","rothschild":"wein","grand sud":"wein","vin de france":"wein","sandeman":"spirituosen","osborne":"spirituosen","nordbrand":"spirituosen","teekanne":"tee","oryza":"reis","leimer":"brot","miracel whip":"soßen","apostels":"soßen","mc cain":"pommes","namdong":"fertiggericht","dovgan":"fertiggericht","satori":"fertiggericht","tönnies":"schwein","axel schulz":"schwein","wilhelm brandenburg":"wurst","golßener":"soßen","nordsee":"fisch","alfrio":"fisch","wurzener":"chips","pom-bär":"chips","bravo":"nüsse","corny":"müsli","little moons":"eis","dr. oetker":"backwaren","uncle sam":"NONFOOD","purina":"NONFOOD","buko":"frischkäse","kiri":"frischkäse","magnum":"eis","ben&jerry":"eis","ben jerry":"eis","mikado":"kekse","prinzenrolle":"kekse","de beukelaer":"kekse","raffaello":"schokolade","maxi king":"schokolade","goldbären":"schokolade","pico-balla":"schokolade","lipton":"limonade","starbucks":"kaffee","karlsberg":"bier","mixery":"bier","landskron":"bier","pülleken":"bier","büble":"bier","wilthener":"spirituosen","bacardi":"spirituosen","nordhäuser":"spirituosen","pircher":"spirituosen","martini":"spirituosen","novantaceppi":"wein","amédée":"wein","nudossi":"marmelade","gutfried":"wurst","dreistern":"wurst","steinhaus":"fleisch","tante fanny":"backwaren","chovi":"soßen","delphi":"konserven","nong shim":"fertiggericht","garden gourmet":"tofu","popp":"soßen","schlichting":"soßen","hipp":"obst","gillette":"NONFOOD","bevola":"NONFOOD","biff":"NONFOOD","finish":"NONFOOD","kitekat":"NONFOOD","medion":"NONFOOD","tefal":"NONFOOD","philips":"NONFOOD","berndes":"NONFOOD","newcential":"NONFOOD","countryside":"NONFOOD","collectino":"NONFOOD","dick & durstig":"NONFOOD","miraball":"NONFOOD","rauch":"saft","happy day":"saft","meica":"bratwurst","becel":"margarine","brunch":"margarine","yogurette":"schokolade","mars":"schokolade","berggold":"schokolade","kathi":"backwaren","keunecke":"fleisch","mühlenhof":"fleisch","windau":"wurst","züger":"frischkäse","zespri":"obst","gösser":"bier","blanchet":"wein","grillo":"wein","tilly":"kekse","the bitery":"kekse","milram":"käse","actimel":"joghurt","vöslauer":"wasser","tulip":"fleisch",
 "zott":"joghurt","bresso":"frischkäse","géramont":"käse","leerdammer":"käse","milkana":"käse",
 "arla":"milch","alpro":"milch","oatly":"milch","exquisa":"frischkäse","almette":"frischkäse","gazi":"käse","rama":"margarine","cremefine":"margarine",
 # Fleisch/Wurst/Fisch
 "reinert":"wurst","rügenwalder":"wurst","herta":"wurst","wiesenhof":"hähnchen","bifi":"wurst",
 "butcher":"rind","k-purland":"fleisch","nadler":"fisch","iglo":"tiefkühlgemüse","frosta":"fertiggericht",
 # Eis
 "mövenpick":"eis","schöller":"eis","nuii":"eis","langnese":"eis","fruity ice":"eis",
 # Soßen/Fertig
 "knorr":"soßen","kühne":"soßen","hellmann":"soßen","homann":"soßen","maggi":"soßen","develey":"soßen",
 "orto mio":"soßen","penny ready":"fertiggericht","bürger":"fertiggericht","san fabio":"pizza",
 "greenland":"tiefkühlgemüse","vitalis":"müsli","kellogg":"müsli","ben's original":"fertiggericht",
 # Spirituosen/Sekt
 "gorbatschow":"spirituosen","cinzano":"spirituosen","baileys":"spirituosen","jägermeister":"spirituosen",
 "mangaroca":"spirituosen","rotkäppchen":"wein","freixenet":"wein",
 # Drogerie
 "nivea":"windeln/hygiene","l'oréal":"windeln/hygiene","garnier":"windeln/hygiene","schwarzkopf":"windeln/hygiene",
 "palmolive":"windeln/hygiene","always":"windeln/hygiene","carefree":"windeln/hygiene","sagrotan":"windeln/hygiene",
 "softlan":"windeln/hygiene","persil":"windeln/hygiene","ariel":"windeln/hygiene","pampers":"windeln/hygiene",
 # Non-Food-Marken
 "crivit":"NONFOOD","silvercrest":"NONFOOD","grundig":"NONFOOD","hammersmith":"NONFOOD","livington":"NONFOOD",
 "kingshill":"NONFOOD","spice&soul":"NONFOOD","wenger":"NONFOOD","tronic":"NONFOOD","brita":"NONFOOD",
 "sodastream":"NONFOOD","trendhaus":"NONFOOD","parkside":"NONFOOD",
}
V["fleisch"] = ([],[],[])
V["obst"] = (["fruchtmix","sommerfrucht","obst","pak choi"],[],[])
V["kakao"] = (["kakao","kakaohaltiges","trinkschokolade"],["kakao"],[])
V["ente"] = (["ente","knusperente","entenbrust"],[],[])

# Erweiterungsrunde 2: Sorten & Begriffe in bestehende Einträge mergen
_ADD = {
 "käse":(["tilsiter","camembert","käsestangen","schmelzkäse"],[]),
 "schwein":(["spare ribs","schälrippchen","jägerschnitzel","cordon"],["rücken","rippchen"]),
 "fleisch":([],["frikadellen"]),
 "fisch":(["surimi","calamares","heringsspezialitäten"],["garnelen"]),
 "backwaren":(["laugenbrezel","kirschtasche","spritzring","donut","madeleines","blätterteig","quarkbällchen","börekstick","eisgebäck"],["croissant","brezel","ciabatta"]),
 "kaffee":(["caffe","barista","kaffeegetränk"],[]),
 "schokolade":(["hanuta","amicelli","lakritz","kaubonbons","lollipops","konfekties","tiramisu"],[]),
 "bier":(["klostergold","lager"],[]),
 "wein":(["bordeaux","chianti","primitivo","zweigelt","cremant","imiglykos","rosato","weinhaltiges"],[]),
 "spirituosen":(["ouzo","metaxa","campari","sherry","veterano","cocktails","bittergetränk"],[]),
 "limonade":(["kombucha","malztrunk","erfrischungsgetränk"],[]),
 "obst":(["kiwi","ananas","mango","sungold"],[]),
 "brokkoli":(["radieschen","porree","chinakohl","pak-choi","zuckermais","rote bete","ingwerstücke"],[]),
 "fertiggericht":(["frühlingsrollen","frühlingsrolle","gua bao","jjigae","antipasti"],["teigtaschen"]),
 "eis":(["mochi","icesticks","raketeneis","stracciatella","eisfrüchte"],[]),
 "butter":(["kräuterbutter"],[]),
 "müsli":(["haferpops","cerealienmix"],[]),
 "soßen":(["ajvar","zaziki","tsatsiki","dip","dips"],[]),
 "brot":(["croutons"],[]),
 "chips":(["krupuk","cheese balls"],[]),
 "pudding":(["puddingpulver"],[]),
}
for _t,(_ex,_sf) in _ADD.items():
    V[_t] = (V[_t][0]+_ex, V[_t][1]+_sf, V[_t][2])  # nur über Markenliste erreichbar (K-Purland etc.)

# Non-Food-Begriffe im Titel (fängt Non-Food in Food-Kategorien wie „Wochenangebote")
NONFOOD_TERMS = re.compile(r"lichterkette|lampion|wäschest|wäscheklammer|wäschekorb|kettensäge|akku|werkzeug|kinderbuch|spielzeug|rosen\b|blumen|pflanze|socken|shorts|shirt|cap\b|hose|schuhe|handtuch|bettwäsche|pfannen?\b|topf\b|löffel|messer|grill\b|kohle|batterie|lampe|leuchte|katzen|hunde|tiernahrung|nassfutter|trockenfutter|snack für|rasenkanten|solar|deko|kissen|matratze|drucker|kopfhörer|wc-|reiniger|megaperls|oxi action|schreibwaren|mikrofon|duschregal|sonnensegel|wäscheparf|karaoke|trinkzubehör|wäschetrockner|weißer riese|sonnenspray|duftspüler|sonnencreme|feuchttücher|servietten|haushaltstücher|klumpstreu|geschirrtücher|platzset|schlafsack|fusselrolle|bügeleisen|glasschüssel|lautsprecher|geräusche-box|fliegengitter|kajak|husarenknöpfchen|lavendel|bilderbuch|wecker|hairstyler|bastelkoffer|kochgeschirr|grillplatte|boombox|fliegenfalle|mottenabwehr|badvorleger|schrubber|kosmetikspiegel|shorty|plaid|fototafel|komfort-bh|pantoletten|spannbetttuch|küchentücher|sneaker|hoodie|bodyspray|deospray|sonnenschutz|dutch oven|gläsersortiment|sonnenschirm|tischdecke|fleece|wellnessbürste|maniküre|pediküre|teppich|taillenslip|haftcreme|wasserballon|doppelwandig|kollagenpulver|corega|axe ", re.I)

# Tokens, bei denen Suffix-Matching generell verboten ist (falsche Komposita)
SUFFIX_STOP = {"reis","preis","schwein","schweine","kreis","eis","wein",
               "hackfleisch","gehacktes","abwaschbecken"}

def norm(s):
    s = s.lower()
    s = re.sub(r"[®*™]", "", s)
    s = s.replace("-", " ")
    s = s.translate(str.maketrans("éèêáàâíìóòúù", "eeeaaaiioouu"))
    s = re.sub(r"[^a-zäöüß\- ]", " ", s)
    return re.sub(r"\s+", " ", s).strip()

def tokens(s):
    base = [t for t in re.split(r"[ \-]", norm(s)) if len(t) > 2]
    extra = [t[:-1] for t in base if len(t) > 4 and t[-1] in "sne"]
    return base + extra


def term_hits(text):
    """Begriffe des Wörterbuchs, die auf einen Angebotstext passen.

    Eigene Funktion, weil docs/feedback-auswertung.py dieselbe Regel braucht,
    um zu bestimmen, welcher Eintrag einen gemeldeten Fehltreffer verursacht
    hat. Zwei Kopien dieser Regel wären genau die Sorte Abweichung, die man
    erst merkt, wenn ein Vorschlag am falschen Eintrag ansetzt.
    """
    toks = tokens(text)
    ntext = norm(text)
    hits = []
    for term,(exact,suffixes,block) in V.items():
        if any(norm(b) in ntext for b in block if " " in b) or any(norm(b) in toks or any(t == norm(b) for t in toks) for b in block):
            continue
        hit = any(norm(e) in toks or (" " in e and norm(e) in ntext) for e in exact) \
           or any(any(t.endswith(norm(sfx)) and t not in SUFFIX_STOP
                      and not any(t == norm(b) for b in block) for t in toks)
                  for sfx in suffixes if len(norm(sfx)) >= 4)
        if hit: hits.append(term)
    return hits


# Ab hier: Messlauf gegen die lokale Nightly-DB. In eine Funktion gefasst,
# damit `V`, `MARKEN`, `norm` und `tokens` importierbar sind, ohne dass ein
# Import die Datenbank anfasst — docs/feedback-auswertung.py braucht genau
# diese Definitionen und darf keine zweite Kopie davon führen.
def main():
    con = sqlite3.connect(DB)
    rows = con.execute("""select o.title, coalesce(o.subtitle,''), coalesce(o.category,''), m.name
                          from offers o join markets m on m.id=o.market_id
                          where o.valid_until >= date('now')""").fetchall()

    stats = Counter(); tagged = defaultdict(list); untagged = []
    for title, sub, cat, market in rows:
        text = f"{title} {sub}"
        toks = tokens(text)
        ntext = norm(text)
        if NONFOOD_CAT.search(cat or "") or NONFOOD_TERMS.search(text):
            stats["nonfood"] += 1; continue
        hits = term_hits(text)
        if not hits:  # Marken-Fallback
            for marke, term in MARKEN.items():
                if norm(marke) and norm(marke) in ntext:
                    if term == "NONFOOD":
                        hits = ["NONFOOD"]
                    else:
                        hits = [term]; stats["via_marke"] += 1
                    break
        if hits == ["NONFOOD"]:
            stats["nonfood"] += 1; continue
        if hits:
            stats["tagged"] += 1
            for h in hits: tagged[h].append((market, title))
        else:
            stats["untagged"] += 1
            untagged.append((market, title, sub, cat))

    total = len(rows)
    print(f"Angebote gültig heute: {total}")
    print(f"Non-Food (per Kategorie erkannt): {stats['nonfood']} ({stats['nonfood']/total:.0%})")
    food = total - stats["nonfood"]
    print(f"Food-Angebote: {food}")
    print(f"  regelbasiert getaggt: {stats['tagged']} ({stats['tagged']/food:.0%})")
    print(f"  ungetaggt:            {stats['untagged']} ({stats['untagged']/food:.0%})")
    print("\n== Treffer pro Begriff (Top 25) ==")
    for term, lst in sorted(tagged.items(), key=lambda x:-len(x[1]))[:25]:
        print(f"  {term:16s} {len(lst):3d}  z.B. {lst[0][1][:60]}")
    print("\n== Ungetaggte Beispiele (50 zufällig) ==")
    import random; random.seed(1)
    for market, title, sub, cat in random.sample(untagged, min(120, len(untagged))):
        print(f"  [{market[:12]:12s}] {title[:55]:55s} | {sub[:25]:25s} | {cat[:25]}")

    json.dump({"begriffe":{t:{"exact":e,"suffix":s,"block":b} for t,(e,s,b) in V.items()},"marken":MARKEN},
              open(os.path.join(os.path.dirname(__file__),"matching-woerterbuch.json"),"w"), ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()
