# Revue indépendante FRONT — visibilité de l'échec broker

Lecture seule du delta gelé dans le worktree broker-service : `front/src/model.ts`, `front/src/main.ts`, `front/tests/model.test.mjs`. Aucun Cargo/npm/test, navigateur, moteur, QA, secret ou accès externe. Le rapport backend demeure séparé et figé ; aucune qualification visuelle n'est déduite de cette revue.

## Conclusion source

**Aucun P0/P1 reproductible identifié dans ces trois fichiers.** Les chemins de preuve GREEN annoncés restent à lire avant clôture des validations.

- `model.ts:112–151` : les Maps privées contiennent neuf phases et treize étapes fermées, dont le label legacy load_after_registration. Le kind doit être exactement broker.batch.failure ; seule la version numérique1 permet la traduction des valeurs connues. Métadonnées absentes, version future/string et valeurs inconnues restent Inconnue, sans appel de conversion de ces valeurs en texte. Les valeurs toString et __proto__ ne récupèrent pas de propriété héritée d'un dictionnaire. Un runtime_effect contradictoire ou libre n'est pas repris : l'effet reste toujours non confirmé.
- `model.ts:154–164` : le rapport précède argv/path/message/reason/task et decision.reason ; les canaris placés dans ces champs ne deviennent pas le résumé. Cela ne transforme aucune donnée frontend en autorité d'exécution. Les seules phase/étape connues sont présentées comme frontières du signalement, pas comme succès du démarrage ou état terminal.
- `model.ts:195–217` : Actions inclut le kind exact même sans commande ; son suffixe .extra n'est pas ajouté par ce nouveau critère. Aucun changement à Security, aux compteurs, aux taux, aux filtres, au chargement ou à la portée tenant. Les critères existants command/decision continuent à s'appliquer aux autres événements, sans prétendre qu'un échec broker est un incident de sécurité.
- `main.ts:1810–1940` : les nouvelles branches Résumé et Commande sont prioritaires sur l'ancienne présentation décision/argv/sorties et précisent que l'état final reste à confirmer. Les libellés contrôlés passent par el/labelValue ; dom.ts:9–13 insère les chaînes avec createTextNode. Aucun nouveau HTML interprété, événement DOM, permission ou requête réseau. Le contenu et les raccourcis des onglets existants sont conservés.

Les métadonnées v1 connues sont traduites indépendamment : une paire phase/étape inhabituelle n'est pas requalifiée en cause ni en effet confirmé. Le helper n'est pas un validateur complet du contrat d'événement. Il ignore les sous-diagnostics natifs/runtime dans cette présentation courte ; l'absence de broker_step demeure Inconnue même si un sous-diagnostic plus précis existe. La vue Données brutes préexistante affiche toujours l'événement reçu ; ce complément n'est pas un nouveau mécanisme de masquage serveur et n'autorise pas à y persister des secrets.

La lecture ordinaire des propriétés de data n'impose pas Object.hasOwn. Un objet fabriqué en JavaScript avec des métadonnées héritées pourrait donc être traduit comme connu ; aucune voie de création de ce prototype à partir des réponses JSON ni pollution de prototype n'est introduite ou établie par ce delta. Le Map lookup des valeurs __proto__/toString est bien fermé. Ce cas ne constitue pas un P1 démontré dans le périmètre.

## Tests et statut de preuve

RED réellement lu : `/private/tmp/ai-guard-frontend-red-20261031-232803/frontend-red.log`. Trois assertions échouent sur le code antérieur : libellé Échec signalé absent, événement absent d'Actions, puis libellé inconnu absent. La compilation du fichier et l'exécution du test atteignent ces assertions. L'assertion de confidentialité du canari n'est pas atteinte en RED ; aucun ancien défaut d'affichage du canari n'est déclaré reproduit par ce log.

Les trois tests ajoutés couvrent résumé/effet, présence Actions sans Security, et inconnus/version future/valeurs héritables comme labels avec canaris. Ils ne sont pas des tests DOM ou navigateur. GREEN modèle11/11 et build TypeScript/Vite ont été annoncés par Root/Luna mais les logs ne sont pas encore recoupés à cette version du rapport : **preuve en attente**. Aucun PASS anticipé ici.

## SHA256 relus

| Fichier | SHA256 |
|---|---|
| front/src/model.ts | 4619641810f983f9ed7439e725eeab7fbe6f664b8b20c2dc31a8afc14b1af1c1 |
| front/src/main.ts | 2ae6c8d9099192f890e5cf08f9ecb8d94a4ce519c4423141c7863bb4460035a6 |
| front/tests/model.test.mjs | 4c51b9a4c33e1848c4fdf7748768d5f9ef68c4ef5464f0dd3a1fdd9a91830427 |

Diffcheck ciblé RC0. Aucun style/thème/logo/serveur modifié dans ce lot. H10 reste FAIL d'observation ; le nouveau code moteur n'est pas exercé par cette revue. Aucun gate runtime fermé. Rendu réel, clavier/focus, contrastes, desktop/mobile et Node24 de référence : non validés par cette lecture source.

## Annexe finale — preuves GREEN terminales

Lecture réelle des logs et fichiers RC dans `/private/tmp/ai-guard-frontend-green-20261031-233407` ; cette annexe remplace l'état de validation « en attente » ci-dessus. Aucun test ou build lancé par le relecteur.

| Cible Luna | Résultat constaté |
|---|---|
| 01-model-tests | RC0, fichier modèle11/11 PASS, 0 échec/ignoré ; dont les trois régressions ajoutées |
| 02-npm-ci-offline | RC0, 15paquets ajoutés en5s ; installation depuis le cache hors ligne selon la commande Luna du lot |
| 03-npm-build | RC0, `tsc --noEmit && vite build`, Vite7.3.7, 10modules transformés, build1,56s |

Environnement relu dans les captures : Nodev25.6.1, npm11.13.0. Node24 de référence et navigateur/QA visuelle : **NOT_RUN**, comme consigné dans final-summary.txt. Les11fonctions correspondent au fichier modèle ciblé ; aucun résultat de tests DOM, de suite navigateur ou d'E2E n'est ajouté à ce compte.

Les trois SHA source ci-dessus sont restés identiques après validation. Le lock relu est `30d12765534dd2b2f89ae78b0b9f180ccc0fbfeb755fac33bd972608c70f2c44`, sans changement dans ce lot. Les rapports Luna vérifient leurs sept fichiers sélectionnés en snapshot et checkout. Pour la couverture complète des sources frontend, les rapports indépendants Root `/private/tmp/ai-guard-front-root-verify.LRd7e2/{checkout-check,extracted-check}.txt` contiennent chacun24lignes OK, couvrant les24fichiers tracked archivés. Ces lectures de preuve n'élargissent pas la revue métier des trois fichiers.

- Manifeste source complet relu : `10c7724d467bfbff590f0168d54c3836b0e3f1a57b9fd18ccd39c67c39845a9f`.
- Archive frontend complète relue : `d3e519eb8cfad7d3910ed0a32bf905e2cc650b5a276d18f44c92c47d6fc0a11f`.
- Manifeste dist relu : `d0acb8e2ad2b28e725d63ac411f12c3335e4895bd14216efb5de79ecf78337a4` ; dist-check.txt vérifie ses quatre fichiers.

Le scan de marqueurs signalait la chaîne fixe Bearer dans le JS. `api.ts:46`, inchangé, la compose avec la variable privée du token pour le header Authorization ; cette occurrence seule ne prouve pas un credential embarqué. La preuve Root dist-known-key-scan.txt indique quatre fichiers examinés, trois sources de secrets connus et zéro fichier contenant ces valeurs littérales. Elle ne démontre pas l'absence de tout secret inconnu ou encodé ; aucune valeur réelle n'a été lue pour cette revue.

**Conclusion indépendante frontend : aucun P0/P1 restant identifié dans le delta gelé ; les trois régressions ont un RED sur assertions fonctionnelles puis le fichier modèle11/11 et le build TypeScript/Vite passent RC0.** Le rapport fermé conserve l'effet non confirmé et les valeurs inconnues sans texte libre dans le résumé et ses nouveaux détails. Actions rend le signalement accessible sans nouveau compteur ni classement automatique en incident de sécurité. La vue brute demeure celle préexistante des données reçues ; aucun masquage universel n'est revendiqué.

Validation visuelle, focus/clavier réel, responsive et thèmes : **NOT_RUN**. Node24 : **NOT_RUN**. Nouvelle émission après échec moteur réel avec ce code : **NOT_RUN** ; H10 reste FAIL d'observation et sa cause inconnue, G09/D5 ouverts. Cette annexe ne change pas le périmètre du rapport backend déjà figé.
