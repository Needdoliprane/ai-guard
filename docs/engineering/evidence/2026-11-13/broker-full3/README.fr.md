# Broker / Full3 — cinq parents de settlement, 13 novembre 2026

[English](README.md) · [Tests et commandes typés](TESTS.json) · [Contrat de supervision](../../../BROKER_SUPERVISION.md) · [Guide broker](../../../../setup/broker/README.fr.md) · [Contrat du raccord relais](../../../../setup/relay/BROKER_HANDOFF.fr.md) · [Contrat API](../../../../api.md)

Cinq parents Rust exacts de règlement normal d'une retraite passent sur la composition figée public+E1. Le build lib-test puis le check toutes cibles passent, avec respectivement **35 diagnostics d'avertissement au build et 343 au check**. Ces résultats logiciels ne qualifient ni Clippy strict, reprise complète, fencing ou runtime réel. Aucun gate production ou constat d'audit historique n'est fermé.

## Source et preuves

Le candidat `223b699e959964bef4460f34857516cb8cdd37c8` est composé sur le public `4cd336c9dd51cc7fa8f81d50ab25788dc32b843f`. Le cadre broker de 136 chemins et sa revue source antérieure précèdent cette composition E1. Les métadonnées de composition Root consignent 134 changements broker effectifs, deux chemins possédés inchangés et les 37 chemins E1 préservés. Le résultat courant appartient à l'archive exacte de 2707 fichiers ci-dessous, avec ses modes retenus ; il ne se transfère pas à un autre checkout ou commit de publication ultérieur.

| Relevé | SHA256 |
|---|---|
| Cadre historique d'entrée de 136 chemins | `d4e4402a898cc6ad728c1bcf807bf69e05ae7e13175c7485a64affd04d0e1a37` |
| Ancienne revue bornée de composition (Markdown) | `32dfd746c74a0198fb9b471cab6a5db044136f42dd7bcd77de908c4a72ee622e` |
| Revue indépendante du plan courant | `b6b0e8668c6f939e724e1c9d75ffc4d7347ec82fab91890fe9dea234dd3d694e` |
| Source d'entrée courante,2707 fichiers /32675840 octets d'archive | `d662f5c7f51b4ce8003614803ed87620484485933f39d251d7550b278136abfe` |
| Résultat final vérifié par Root | `2bc5dab9d4934fc98b34013554c11a45592c08faf572f73c4435706f85773412` |
| Source après exécution,32860160 octets d'archive | `1df33e3943dea9acf66e877a681fdff410576e0e5f787f11a04ef9246d7550c7` |
| Manifeste privé des293 pièces brutes | `db712410daaef48d3a4e67f94c793c43e77f5ff7e8ff8477eb30bf131934db08` |

Archives source, revues originales, traces brutes, fixtures et credentials restent hors Git. Ce résumé public ne contient ni contenu brut de fixture, chemin privé hôte ou réponse API. Les statistiques des fixtures proviennent du reçu final vérifié par Root ; le rédacteur de publication n'a ni lu, ni décodé, ni extrait leurs payloads.

## Cinq parents constatés

Les cinq rendent RC0, un passé, zéro échec et zéro ignoré, après une liste fraîche exacte d'un test. Leurs sélecteurs complets et vrais argv figurent dans [TESTS.json](TESTS.json).

| Cas | Comportement borné exercé |
|---|---|
| Final original soumis après un résultat Q indisponible | La retraite originale se règle sans créer un nouveau Final. |
| Même Final public d'un autre core | Les données publiques d'un core étranger ne permettent ni query ni clear du keeper original. |
| Slot local absent sans complétion opaque | L'absence seule conserve le keeper original. |
| Input vérifié déjà retenu | Un autre wrapper de query est refusé. |
| Cleanup confirmé après annulation | Génération préservée et admission toujours fermée. |

Les fixtures admises utilisent de vrais fichiers privés Journal/SQLite/configuration/audit, un gateway loopback possédé, de l'IPC Unix privé et des joins d'acteurs dans le même processus. La substitution de test `SyntheticReady` retourne avant le chemin runtime métier. Aucune commande worker, moteur ou fournisseur n'est invoquée. Une donnée d'invocation nommant un programme ne prouve pas son exécution. Ces cinq cas n'exercent ni redémarrage complet du broker, ni l'ensemble de la reprise Full3/relais.

## Commandes et collectes

Luna exécute une seule campagne admise ; Root vérifie indépendamment résultat, égalité source, captures et fermeture. Dans l'environnement QA hors ligne, les vrais argv de compilation/check sont :

```console
/usr/local/cargo/bin/cargo test --frozen --offline --lib --no-run --message-format=json
/usr/local/cargo/bin/cargo check --frozen --offline --all-targets --message-format=json
```

Le harnais résout le vrai exécutable lib-test depuis le JSON Cargo. Il liste chaque sélecteur exact avec `--list --exact`, puis l'invoque une fois avec `--exact --nocapture --test-threads=1`, sous une borne de120 secondes. Aucun CLI principal ou autre parent n'est exécuté. Le check toutes cibles vient après les cinq PASS seulement. Une compilation toutes cibles ne constitue ni Clippy strict, ni résultat de suite complète.

Le conteneur QA est Linux ARM64 (`7.0.14-linuxkit`), sélection Rust `1.99.1`, image `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`, UID/GID1000, réseau `none`, racine readonly, capacités supprimées, no-new-privileges, IPC/cgroup privés,4 CPU,9 Gio de mémoire sans swap supplémentaire et128 PID. Il possède trois tmpfs, dont un scratch privé de4 Gio, et seulement deux binds d'entrées/cache readonly. Ces faits QA ne qualifient ni workers métier, ni adaptateurs runtime.

Six collectes finales aboutissent : source exacte après exécution, export source borné, inventaire régulier des fixtures, export privé opaque des fixtures, observations kernel et cgroup. Root vérifie contenus et modes des2707 fichiers après exécution. La capture privée compte **96 fichiers réguliers /2117346 octets utiles**, dans une **archive de2201600 octets en mode0600**, SHA256 `23251f802d3f06071ed7b6962df2472cff767008803596ce2bb8915092c319dd`. Elle n'est pas distribuée. Les deux pipelines de capture source/fixtures conservent bornes32 Mio et contrôles de débordement.

Les six collectes et stop/remove rendent zéro ; fermeture par CID exact RC0, conteneur supprimé, `oom=0`, `oom_kill=0` et `OOMKilled=false`. Le code137 du holder reste distinct des RC0 tests/check et de la fermeture. Le green provisoire de l'exécuteur n'est accepté qu'après vérification Root.

## Historique et exigences ouvertes

Le parent prewire antérieur passe une fois sur une autre source de2668 fichiers (`0d393fff…4725`), reçu `d0e3584d95ea67777a129696b7d6393d2d44ce84587c90c3c576ed31b5486877`. Il teste un cas normal listener/prewire avec observation runtime synthétique. Il reste distinct et n'est pas un sixième PASS ici. Le cadre136 figé à04:06 UTC conserve **cinq tests privés de settlement NOT_RUN** sur sa source alors privée `ceda17d8…94bd` ; la campagne composée courante ne réécrit pas ce statut historique. Les familles d'opérations namespace/restauration et NativeResume précédemment exclues ne sont ni exécutées ni reconstruites par cette campagne.

Les six guides décrivent des contrats plus larges. La réconciliation historique de l'API ne crée ni nouveau permis ni preuve de join. L'événement d'audit broker Resume n'est qu'une demande ; il ne reconstruit ni owner original ni timer depuis JSON. La fermeture relais exige preuves durables exactes et join local ; un handle absent ou un worker terminal ne l'établit pas seul. Ce relevé ne ferme aucune de ces exigences d'acceptation plus larges.

Fencing général, reprise froide/complète, restauration de namespace, transport relais, exécution réelle worker/moteur/modèle/fournisseur, confinement réseau inter-tenant, acceptation Docker/Podman/Kubernetes, reprise stockage/powerloss, capacité100 et parcours opérateur complet restent non qualifiés par ces cinq parents. Clippy strict, suite complète et build release ne sont pas exécutés dans cette campagne. Aucun SLO de durée, garantie de confinement de toute plateforme ou préparation production n'en découle.
