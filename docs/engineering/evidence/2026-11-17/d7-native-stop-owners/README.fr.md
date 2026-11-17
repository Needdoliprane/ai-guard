# D7 : propriété STOP native — preuves logicielles

[English](README.md) · [Résultats et empreintes](TESTS.json) ·
[Contrat D7](../../../D7_STOP_RECOVERY.fr.md). Relevé du 17 novembre 2026.

La source R5 est le commit `6a32771ce76961f45d163767879d4ba08eeabcec`, gelé
avec 2 964 fichiers. SHA256 de l'archive :
`6c5554cd211c93abf44584b353dfef0bba04f9c975ec0ef920536dcfb6c24a19`.
La revue source donne GO_SOURCE_ONLY ; les résultats d'exécution ont leurs
propres reçus.

Une réservation Legacy réellement neuve reçoit son ACK après installation du
vrai thread et du contexte runtime natif, du snapshot STOP exact et de la lease
du rôle. Bind garde ce propriétaire au lieu de créer un autre acteur de start. Un seul registre
borne la capacité Original neuve à S et le total des cellules propriétaires
natives à S+W.
Avec W>0, une cellule de rattrapage laisse S+max(W−1,0) cellules permanentes.
Comptes et santé du transport ne prouvent ni arrêt d'un conteneur ni collecte IO.

Les nouveaux rôles Legacy cold/existing démarrent fenced. Un acquittement de
couverture active exige le propriétaire STOP natif réservé correspondant ;
un refus ou une couverture inconnue n'autorise aucun nouveau start et ne
remplace pas l'ancien daemon. Le correctif R5 publie Pending
à sa première création seulement : une reprise relit strictement le Pending
existant et compare son identité sans révision vide. Un état absent/divergent
refuse avant la RPC Arm. Aucune atomicité CAS à travers l'attente native n'est
revendiquée.

**Seize tests lib ciblés R5 ont réellement été exécutés et ont tous réussi**, chacun
avec son oracle exact d'un test. Compilation et listes binaires ont été vérifiées
indépendamment. Ce n'est pas toute la matrice ; les PASS de sources différentes
ne s'additionnent pas.

| Campagne | Sélection | PASS | FAIL | NOT_RUN |
|---|---|---:|---:|---:|
| R5, `6a32771` | Cas lib ciblés | 16 | 0 | 0 |
| R4, `5b82fdd` | Campagne CLI/lib distincte | 4 | 1 | 15 |
| R2, `a3c5370` | Campagne lib historique | 24 | 1 | 17 |

R4 comprend un vrai cas de compatibilité ancien V1, explicitement invoqué et
PASS ; il n'a pas été rejoué sur R5. Les anciens FAIL et valeurs individuelles
non capturées restent historiques, sans valeur de remplacement déduite.
TESTS.json donne les FQN exacts et les empreintes des reçus.

Clippy R5 all-targets a rendu 0 en mode non strict : 561 émissions de warnings et zéro erreur. Sans baseline comparable, aucun n'est
classé nouveau. La gate Clippy stricte reste ouverte.

**Ces preuves clôturent 0/12 gates de production et aucun fingerprint global
d'audit.** Réouverture métier cold automatique, Pending sans owner retenu,
retraite/Q Legacy et libération de quota restent ouverts. Effets STOP physiques,
échéances/collecte IO, vrais moteurs Docker/Podman, Kubernetes et échelle de 100
workers ne sont pas qualifiés. Les bornes d'octets/lecture ne garantissent ni
latence stderr synchrone ni volume du writer. Ces résultats logiciels ne
constituent pas une qualification production.
