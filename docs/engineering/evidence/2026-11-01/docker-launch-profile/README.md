# Profil Docker de lancement — contrat et validation

État courant du 1er novembre : douze fonctions uniques PASS au gel après correction du défaut de projection STOP sous blob surdimensionné. Une indirection privée supplémentaire corrige les deux lints Clippy `large_enum_variant` ; ce delta est relu, Clippy strict, release, son canari de garde et trois canaris canoniques passent. Le FAIL voisin EOF et son rejeu diagnostique restent conservés sans cause attribuée. L'oracle renforcé puis l'intégration logicielle ciblée PRIMARY passent ensuite : EOF1/1, configuration fournisseur10/10, formatage et Clippy strict, sources concordantes. Aucun moteur ni D5/fencing qualifié. Les paragraphes ci-dessous conservent les gels historiques et leurs limites, sans transférer leurs résultats à un autre run.

Base `82f00d751ed233e1282505000891bad49164ffd0`, checkout frère `docker-launch-profile`, branche `codex/docker-launch-profile`. Le [contrat concret](contract-before-red.md) confronte la [proposition source initiale](../journal-cost/d5-launch-profile-design.md) au code : intention et credentials V1 inchangés, nouveau couple bindings/Accepted V2, choix opérateur explicite, engagement du scope authentifié et du parent accepté, seed distinct et profil privé pour les starts préarmés. Aucun service, stockage, transport ou identité supplémentaire.

La [revue indépendante](contract-review.md) demande quatre précisions, intégrées avant approbation Root : NUL unique du domaine, preuve engagée distincte du blob pour STOP, inventaire fermé/budgets partiels et validation des helpers avant worker principal. Les STOP prouvés ne dépendent pas du contenu du profil ; un budget ou audit devenu inconnu interdit toutefois un faux ACK d'append. Cet écart reste explicite, sans réserve ni contournement implicite. Un ancien lecteur refuse les nouveaux formats, même dans l'historique conservé ; aucun effacement ou rollback pendant des effets possibles n'est autorisé.

Le [premier test RED](registry-red-plan.md) utilise les API actuelles : contrôle positif V1 puis attente d'acceptation V2 stricte, fichiers JSON synthétiques privés, aucune ouverture du programme/token/endpoint référencé. Le [patch du test](registry-red.patch) permet de reproduire cette absence du parser V2 ; il n'est pas appliqué au code de broker-service.

**RED attendu constaté**, RC101 : le contrôle positif V1 passe, puis l'assertion V2 échoue à la ligne445, après compilation3min54s et exécution0,02s. Les [sorties](registry-red/01-registry-v2-red.log), [commande préalable](registry-red/command-before-run.txt) et160 sources pré/post concordantes sont conservées ; Root vérifie aussi l'[archive extraite](registry-red/root-archive-verify.txt). Un échec plus tôt n'aurait pas été une baseline acceptée. Le seul source modifié du checkout frère est ce test cfg(test), SHA `54498cd3585de8fa03efeef2b831562a9c6add5a45baf97eef339dc507c8bfe5` ; aucun corps de production changé.

À ce premier gel RED, l'implémentation et la qualification étaient absentes. L'avancement actuel est décrit en tête. Interopérabilité CLI/moteur, témoin D5, reprise et fencing restent non qualifiés ; aucun lancement, retrait de slot ou gate clos n'est déduit des tests logiciels.

## Correction du défaut STOP et limites des preuves

La [revue de l'implémentation](implementation-review.md) avait trouvé une propagation immédiate du dépassement de taille du blob avant reconstruction des records/Arm. Le [RED réel](oversize-red/cargo-red.log), RC101 après contrôle positif, le reproduit. Le [correctif relu](oversize-fix-review.md) sépare uniquement les octets metadata des blobs du budget dur des records : projections vérifiées et siblings sont conservés avec diagnostic incomplet/limite, sans lever les caps d'append ou inventer un ACK. Full/start restent stricts. Les trois variantes sparse job/scope/1TiB de la nouvelle régression passent ; aucune lecture/allocation proportionnelle à cette taille n'est revendiquée.

Les douze fonctions sont exécutées sur 162 sources gelées ; la première via Cargo, les onze suivantes via copie privée vérifiée du même binaire. Elles utilisent de vrais fichiers/Journal/Store et des snapshots runtime explicitement synthétiques ; aucune commande moteur n'est exécutée. Le delta Clippy ne change ensuite que l'indirection privée du binding attendu dans `PrearmedStartControl`, avec égalité complète et contrôle final adjacent au spawn. Les formats persistés ne changent pas. Aucun ancien résultat n'est présenté comme un nouveau run.

## Contrôles après l'indirection privée

Le manifeste [source-final.sha256](source-final.sha256) fixe les 18 sources applicatives ; `runtime.rs` vaut `adddd62b03c85b0869caf29d9c8ec2cee82c224059c9a2fc42e0d237aad348ad`. Le pipeline conserve 162 sources identiques avant/après et des archives portables sans AppleDouble. Capture privée `/private/tmp/ai-guard-launch-profile-size-fix-20261101T031334Z/`.

| Contrôle | Résultat constaté |
|---|---|
| Formatage Rust | PASS |
| Clippy all-targets, warnings refusés | PASS, 1 min 49 s |
| Canari de comparaison complète du binding | PASS, 1 fonction |
| Build release verrouillé | PASS, 11 min 58 s |
| Canaris canoniques V2, V3 et V4 | PASS, 3 fonctions uniques |
| `runtime_prearmed` | FAIL, 3 PASS et 1 FAIL avant entrée du callback EOF |

Le FAIL `pending_started_callback_is_not_reinvoked_when_attach_reaches_eof` atteint le délai de préparation historique de 5 s ; la propriété EOF n'est pas exercée. Le seul rejeu instrumenté passe en 8,23 s, sans diagnostic déclenché, et n'établit donc pas la cause du premier timeout. La revue trouve aussi une faiblesse de l'ancien oracle : un marqueur écrit juste avant la fin de l'attach et une attente arbitraire ne prouvent pas que le runtime a consommé EOF. La régression remplace cette attente par une inspection terminale causalement située après EOF et vérifie que le premier futur du callback reste présent puis termine. Les budgets de harnais sont distincts des plafonds produit. Cette nouvelle régression et l'intégration PRIMARY restent NOT_RUN à ce point.

## Intégration PRIMARY — contrôle isolé final

Les 18 sources du manifeste final sont importées sans changement dans `broker-service`. Le test EOF renforcé, SHA `cea374b9e71815f148265cc44bb735a4f1820d036ce593ff0f14448771c2e59c`, remplace l'attente arbitraire par deux preuves : inspection terminale après consommation de la fin du transport pendant que le premier futur reste bloqué ; puis fin de ce même futur après notification, avec un seul appel total et les résultats exacts conservés. Sa revue Root est distincte de l'implémentation du sous-agent. Les budgets de harnais couvrent séparément les inspections externes ; aucun timeout runtime/Arm n'est changé et la cause du FAIL historique reste inconnue.

Luna compile dans une cible privée neuve propre à PRIMARY : `/private/tmp/ai-guard-primary-provider-eof-20261101T041000Z/target`. Formatage PASS ; EOF exact **1/1 PASS en 8,80 s**, 3 filtrés ; Clippy strict all-targets PASS en 1 min 59 s. Les dix tests de configuration fournisseur, dont le correctif de nom Claude indépendant, passent aussi dans cette même cible. Les 161 sources du relevé avant/après et les 18 hashes du manifeste P0 concordent. Binaire EOF : `07e5857b8992e69c2dd9cb0289adfdbd9ba471f8eacf72ded17f74427e5893a5`. Aucun nouveau release ni rejeu des douze longues régressions n'est exécuté dans cette cible ; leurs résultats restent attachés aux gels précédents.

Deux arrêts avant exécution EOF sont conservés dans cette capture : PATH incomplet puis mode accidentel `0600` de la seule racine du target privé. Le préflight confirme SDK/compilateur ; seule cette racine est rétablie en `0700`. Aucun cache partagé ou source n'est supprimé/modifié pour obtenir le PASS. L'incohérence de bibliothèque constatée auparavant dans le cache partagé reste non attribuée ; le contrôle isolé ne la prouve pas rétrospectivement. Aucun moteur, D5 ou G09 n'est qualifié.
