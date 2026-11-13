# Composition Claude A/B — 13 novembre 2026

[English](README.md) · [Résultats typés](TESTS.json)

**Les cibles logicielles propres à cette source passent, avec un sous-cas explicitement non couvert. Le harnais global sort 99 ; ce n’est pas un PASS global.** Luna a exécuté une campagne admise dans un conteneur outil QA Linux ARM64. Son image a été inspectée ; aucune image produit contrôle/worker n’a été construite ou qualifiée.

## Source et périmètre

Commit immuable testé : `f12a46af435a09c754084f3a1eb3db9f650f800f`, arbre `787590a570920a742996559a7ea395ec6737fb49` : 2738 fichiers, manifeste d’entrée SHA-256 `3056861835293268fe182b266b62eafce90a705eb304b0325359adb04e1a941d`. Composant Kubernetes A : `356312ff1f52d231f980c412643b96e73ab245f2` (24 chemins) ; composant release B : `5e8e1811cf7828b12b8a7f94820070606f19f332` (22 chemins). Les 46 postimages livrées concordent avant les quatre mises à jour d’état des guides courants. La source broker `223b699e959964bef4460f34857516cb8cdd37c8` diffère du public Root `6cc6b3a25f4dd1fea4ead3180cde1d58fb912001` sur huit chemins de documentation/preuves seulement. Séparément, la source jointe testée `f12a46af435a09c754084f3a1eb3db9f650f800f` contient les 46 postimages A/B livrées et diffère de Root sur 54 chemins, dont 40 hors docs. La publication prévue compose Root avec ces 46 postimages A/B et les 15 payloads documentaires revus ; Root doit vérifier indépendamment la composition hors documentation après staging, avant commit. Aucune égalité d’arbre complet entre source testée et publication future n’est affirmée.

## Résultats observés

| Contrôle | Résultat observé |
|---|---|
| Compilation | PASS, RC0, zéro erreur compilateur, 275 avertissements |
| 27 groupes sélectionnés uniques | 272 parents actifs distincts PASS : 144 Kubernetes, 123 release, 5 broker ; zéro échec ; deux helpers ignorés restent NOT_RUN |
| Listes fraîches et verdicts de groupe | 27 listes et 27 verdicts, tous RC0 |
| Check toutes cibles | PASS, RC0, zéro erreur compilateur, 286 avertissements |
| Harnais global | RC99 ; couverture complète des sous-cas non établie |
| `it-manifest` : parent privé appartenant à un autre UID | NOT_RUN : second compte requis indisponible ; le parent PASS ne couvre pas ce sous-cas |
| Clippy | NOT_RUN |

[TESTS.json](TESTS.json) détaille les groupes. Les commandes release manifest/verify sont le logiciel livré W1–W3, exercé sur de vrais fichiers synthétiques et processus CLI. Ce n’est ni un build release réel, ni une provenance signée, ni une preuve de build reproductible. PW1/PW2/PW3/PW6/PW8 restent proposés et non appliqués. Les labels d’image restent `declared`.

## Conservation et fermeture

Les 2738 contenus et modes post-run concordent exactement avec les entrées source attendues (manifeste post-run SHA-256 `66c70f708e3741fe685d6f1c4eda1b9fe41b94e2b1f82eeb10ca4b148521b41c`, 33832960 octets). Sa sérialisation est distincte du manifeste d’entrée ; leurs octets de manifeste ne sont pas déclarés égaux. Le manifeste des 950 pièces brutes porte le SHA-256 `f0a35767e2bc46157e961c225cde80aedcf7edf2964bb7574262e593862f1098`. Six collectes et la fermeture sortent RC0 ; le conteneur QA exact est retiré, compteurs OOM à zéro et stockage temporaire vide. Fixtures et traces brutes restent privées, exclues de la publication. SHA-256 du reçu vérifié indépendamment : `572cdb995aaffa89229b0bce188fc4f29e9348cd1619414a22da79b18c3c1058`.

## Limites et historique

Aucun vrai worker/runtime métier, fournisseur, IdP, cloud, cluster, macOS natif, charge100, image produit ou build release n’a été exercé. Les effets production S5 restent fermés ; Claude conserve le lot attach S2b actif, non intégré ici. Les questions historiques Q1/Q6 du guide Kubernetes restent séparées du nouveau dispatch ; aucune acceptation anticipée du travail Claude. Reprise/fencing broker complets, relais runtime et gates production restent ouverts. Aucun fingerprint d’audit clos.

Les [preuves Kubernetes initiales](../../2026-11-12/kubernetes-v2-runtime/README.md) et [preuves release initiales](../../2026-11-12/release-integrity/README.md) conservent leur périmètre NOT_RUN daté. Guides courants : [Kubernetes V2](../../../../setup/kubernetes-runtime/README.fr.md) et [métadonnées release](../../../../setup/release/README.fr.md).
