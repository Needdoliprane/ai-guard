# Store15 de politiques E1 — preuves logicielles, 13 novembre 2026

[English](README.md) · [Guide interne](../../../EGRESS_POLICY_STORE.fr.md) · [Commandes/résultats typés](TESTS.json) · [Exigences egress complètes](../../../EGRESS_CONTROL.md)

Le registre Rust du tenant conserve révisions immuables, heads CAS, reçus d'opération exacts et réserves terminales logiques dans sa base SQLite existante. Une maintenance explicite avec sauvegarde installe le format15 depuis le format14. L'acteur de ces opérations privées est un contrôle machine authentifié ; le propriétaire humain de son binding n'est pas un approbateur. Ce lot n'ajoute ni permission réseau humaine, commande CLI, route HTTP, permis de connexion, résolution DNS, socket ou révocation appliquée au runtime. Aucun projet ou base existante d'utilisateur n'a été migré.

Résultats constatés : **14 parents exacts PASS sur R5**, puis **13 parents exacts PASS et check toutes cibles PASS sur R7**. Ces campagnes distinctes portent sur deux compositions figées différentes. R7 ne revendique pas 27 PASS. Son check toutes cibles produit **320 diagnostics d'avertissement**, sans erreur ; ce n'est pas un résultat Clippy strict. Aucun gate production ou constat d'audit historique n'est fermé. G05/G07/G11 restent ouverts ; le réseau bridge existant reste non médié.

## Sources et résultats constatés

Les deux compositions contiennent 2630 fichiers sur la base publique `ceb7b1f1c298a0f31044eca20a09cb2101c4d7c2`, les 29 postimages E1 possédées et quatre correctifs de source humaine revus. Ces quatre correctifs sont publiés séparément dans `328f3bc5cc042ebf7f8924c54d4e717d1de94acb` ; les preuves QA n'affirment pas l'égalité avec un arbre publié ultérieur. R7 ne change que le corps du parent15 dans `src/audit/egress/tests.rs` par rapport à R5. APP, helpers, quatre fichiers humains et 14 premiers corps restent inchangés, après revue indépendante. Le worktree E1 reste sur sa base d'origine ; il n'est pas la source QA composée.

| Relevé figé | Résultat |
|---|---|
| R5, E1 R4 + quatre postimages humaines | Build lib-test PASS ; 14 parents E1 PASS ; parent15 FAIL101 de fixture avant la boucle de casse. Les 12 parents restants et le check toutes cibles sont NOT_RUN. |
| R7, R5 + parent15 E1 R6 revu | Build lib-test PASS ; parent15 corrigé + quatre parents E1 restants + huit canaris humains : 13 PASS, zéro échec/ignoré. Check toutes cibles RC0, zéro diagnostic d'erreur, 320 d'avertissement. Les 14 parents R5 ne sont pas rejoués. |

| Pièce conservée privée | SHA256 |
|---|---|
| Manifeste E1 R6 de 29 fichiers | `a8e87d5348df7863b96a96c57ddbcd55fd45536c04c49f1467c5614ad76ae5a6` |
| Reçu de revue source bornée R6 | `b542662689630be40064e6101f61709e5a02c188a5433919f27e0b591a32eaa6` |
| Archive source d'entrée R5 | `430965dfc57ac5002cde906187224840276eeddce42fcaba6721aec9cf2e5fe9` |
| Reçu de résultat vérifié R5 | `ef0b8870361e8652a3cb134978013991a4f61e9b3289e1cec24b58d7b23b4858` |
| Archive source d'entrée R7 | `dd5f66a927e7ce61727530f0774eb4a4dc79e024dd316d91ff92fc58df8385ae` |
| Reçu de résultat vérifié R7 | `cca283a376daa86709e1b0f540c98b606051db3385e79f9cb7b8be6a6bf56296` |
| Export source borné après exécution R7 | `6c766232b3d7c6f2899ee64367976e9cd79b7238dc77e9d93555c9a154b085fb` |
| Manifeste privé des 468 pièces brutes R7 | `49d31a23923b5abdf726bf880c2e8019951fa37ffb004f0e68a83f054edc5708` |

Archives, revues originales, manifestes, reçus et traces brutes restent privés hors Git. Ce relevé public est un résumé expurgé, sans redistribution de ces pièces. [TESTS.json](TESTS.json) conserve les autres empreintes historiques et les argv exacts par parent, sans chemin hôte, credential ou log brut.

## Historique des échecs conservé

- **R1 NO_GO_SOURCE** : traitement de casse de la famille réservée15 et oracles de saturation/exclusion du propriétaire à corriger. La nouvelle revue indépendante bornée R2 rend GO_SOURCE, sans prouver compilation ou comportement.
- **Compilation R2 FAIL** : 11 erreurs limitées à la fixture ; tests NOT_RUN. R3 corrige imports, `Eq` incompatible et conversions vérifiées d'entiers SQLite. Il ne s'agit pas d'une RED comportementale.
- **Fixture R3 FAIL** : build lib-test PASS, puis premier parent en échec sur `reopened.integrity()`. Installation, assertion d'état actif et réouverture avec vraie source avaient abouti. Le reçu initial attribuait mal l'opération ; sa correction conservée identifie la garde d'intégrité legacy, sans expiration ou échec d'installation. Les 18 autres parents et le check restent NOT_RUN. R4 remplace ce diagnostic par une vraie vérification intégrale scoped et conserve les refus exacts legacy/SourceB avec snapshot inchangé.
- **Fixture R5 FAIL après 14 PASS** : parent15 ne garantit pas la création privée du répertoire. Mode numérique/UID effectifs non capturés ; aucun mode runtime ou umask n'est déduit. L'échec survient à l'ouverture Store avant les kinds réservés.
- **Correction R6 limitée au test** : répertoire neuf `Builder.permissions(0700)` et assertion du mode réel avant l'ouverture Store. Append ordinaire/intégrité positifs avant et après, motifs exacts des refus réservés et snapshots format/head/hash/compte/quota/absence de schéma egress inchangés empêchent qu'un échec générique satisfasse l'oracle. Aucun chemin existant réparé, garde production, délai ou umask modifié. La revue bornée confirme que seul ce corps change.

## Contrats exercés

R5 emploie de vrais Store SQLite privés, sauvegardes/migrations13→14→15, propriétaires/époques source retenus, enveloppe/index privé de credential et bindings review actifs. Il couvre séparation A/B, réouverture/replay durables, rejeu identique en octets et conflit de contenu divergent, CAS, Session active et plafonds tenant historiques, réserves terminales, saturation ordinaire/contrôle/identité par nombre d'événements, rollback SQLite FULL, corruption masquage/projections, exclusion du propriétaire actif, replay de famille réservée et ordre de genèse. Le négatif SourceB utilise l'autre vraie source et vérifie l'absence de mutation.

L'oracle de saturation observe un remplissage positif accepté, le plafond d'événements calculé indépendamment, le refus typé exact, compte réel de lignes et accord octets sérialisés/quota, puis un rejeu identique sans effet. Révoquer un head parmi deux préserve la réserve de l'autre. Il ne mesure pas une saturation physique en octets. La maintenance exige `OwnerContended` exact à l'acquisition, aucun effet sauvegarde/DDL/événement et format14/schéma/head/quota inchangés ; `ConfigurationMismatch` reste distinct. Libérer le même propriétaire permet au même plan retenu de réaliser sauvegarde, migration, reprise et réouverture.

R7 exerce le parent corrigé de refus réservé avant migration, le refus de rejeu après révocation du binding, la régression d'horloge durable, le rejet des bornes entiers/limites et la transition terminale pure d'un head expiré sur projection réellement retenue. Ses huit voisins de source humaine couvrent binding physique parent/entrée, substitution, catégories structurées d'échec parent/entrée, canari de classification de pénurie de descripteurs, gardes entrée privée/alias, isolation d'époque expirée et revalidation du token retenu sous une époque d'écriture existante. Ils n'épuisent pas réellement les FD et ne qualifient aucun runtime.

## Exécution et fermeture

Le conteneur QA hors ligne admis est Linux ARM64 (`7.0.14-linuxkit`), sélection de toolchain Rust `1.99.1`, image `sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`, réseau `none`, racine readonly, non privilégié, IPC/cgroup privés, capacités supprimées, no-new-privileges, 4 CPU, mémoire 9 Gio sans swap supplémentaire, 128 PID et tmpfs scratch borné à 4 Gio. Sources d'entrée et cache public de dépendances sont readonly. Ce profil est celui de la QA, sans qualification worker/runtime. Luna exécute la campagne ciblée admise ; Root vérifie les reçus et preuves source/fermeture.

Les vrais argv de compilation/check sont :

```console
/usr/local/cargo/bin/cargo test --frozen --offline --lib --no-run --message-format=json
/usr/local/cargo/bin/cargo check --frozen --offline --all-targets --message-format=json
```

Le harnais résout le vrai exécutable lib-test depuis le JSON Cargo, liste chaque sélecteur atteint avec `--exact --list`, puis l'exécute une fois avec `--exact --nocapture --test-threads=1`, en s'arrêtant au premier échec. [TESTS.json](TESTS.json) conserve ces argv réels, l'ordre des sélecteurs et les délais. R5 n'atteint pas le check ; R7 le lance après les 13 sélections restantes PASS. Ces relevés reproduisent l'environnement QA admis sans autoriser une exécution native du projet.

Les deux campagnes vérifient leurs 2630 fichiers source et modes après exécution contre leurs entrées respectives et conservent les exports bornés. R5 conserve 518 pièces brutes ; R7 en conserve 468. Les deux conteneurs QA exacts sont arrêtés/supprimés avec **closure RC0**, `oom=0`, `oom_kill=0` et `OOMKilled=false`. Le pic de mémoire cgroup observé est de 6530342912 octets sur R5 et 6482698240 sur R7. Le code137 du holder reste distinct des codes tests/check et du succès de fermeture. Ces mesures ne bornent pas la RAM d'un processus et ne prouvent pas la capacité100.

## Exigences ouvertes

Le format14 et l'accès legacy restent fail-closed. Le format15 exige maintenance explicite avec consommateurs arrêtés, sauvegarde neuve vérifiée, autorité source retenue, commit explicite et réouverture ; aucune migration automatique ou CLI n'est fournie. La réserve terminale est un comptage logique, sans préallocation disque : SQLite FULL, I/O ou échec de commit peuvent encore empêcher un revoke. `revoke_requested` n'est pas une fermeture de connexion observée.

Reprise après faute commit/WAL/I/O, test Store après expiration réelle, saturation physique en octets, capacité100, permission/diffusion humaine de politique, HTTP/UI, permis de connexion durables, médiation DNS/socket, canal commun/bootstrap, révocation appliquée, observation indépendante des tentatives directes, périmètre Web/MCP distant, quatre modes fournisseur réels et acceptation Docker/Podman/Kubernetes restent ouverts. Clippy strict, suite complète, build release et qualification hôte natif ne sont pas exécutés dans ces campagnes. Aucune activation Internet, action fournisseur/IdP, capteur, MITM/PKI ou cluster n'a lieu.
