# Revue documentation et provenance — checkpoint broker-service

31 octobre 2026. Lecture seule de `.worktrees/broker-service` : README de preuve, provenance, source.sha256, journaux PASS/FAIL, trois rapports, deltas CURRENT_STATE/VALIDATION/TEST_STRATEGY/IMPLEMENTATION_PLAN/BROKER_JOURNAL/BROKER_SUPERVISION. Aucun document du dépôt, source, test ou binaire modifié ; aucun Cargo, runtime ou fournisseur exécuté. Seul ce rapport privé est créé.

## Conclusion

**Pas d'incohérence bloquante de compte, de hash ou de portée trouvée.** Le bilan correspond à un checkpoint de développement avec 40 fonctions uniques ciblées, et non à une verticale ou une release qualifiée. Deux points éditoriaux mineurs restent à clarifier ci-dessous. Aucun test non exécuté n'est transformé en PASS dans le bilan courant examiné.

## Recoupes constatées

### Compte des tests et profils

Les noms `test … ok` des journaux PASS ont été dédupliqués. Le helper `broker::journal::tests::writer_lock_race_child`, invoqué explicitement par son parent, est exclu ; `crash_child_at_stage` et `writer_lock_race_child` ignorés dans la suite parent ne sont pas additionnés. Les trois IPC finaux sont les mêmes fonctions que le premier journal IPC, pas trois nouvelles fonctions.

| Groupe unique | Fonctions |
|---|---:|
| IPC/CLI/writer intégrations | 5 |
| Child natif vide | 1 |
| Ownership writer unitaire | 2 |
| Service unitaire | 10 |
| Workspace autorisé | 4 |
| Outbox | 5 |
| Checkpoint/STOP | 6 |
| Bindings manuel/fournisseur | 2 |
| Parents unitaires Journal | 5 |
| **Total unique** | **40** |

Les logs montrent `Finished test profile [unoptimized + debuginfo]` pour workspace et les groupes debug ; `Finished release profile [optimized]` pour outbox et checkpoint. Le README et TEST_STRATEGY décrivent correctement ces profils. `final-ipc.log`, `final-service.log` et `final-outbox.log` confirment les canaris 3/10/5 après corrections mécaniques, sans prétendre rejouer les autres groupes au même gel.

Les journaux Clippy et build se terminent avec succès, sans diagnostic dans le log Clippy final. Les commandes de la table correspondent aux cibles/filtres et profils consignés. Les logs Cargo sont des sorties de commandes, pas une attestation cryptographique de chaque argument ; cette revue ne rejoue aucune commande Cargo. Le formatage silencieux est rapporté comme résultat de l'orchestration, sans inventer de journal fmt dans le dossier publié.

### Provenance 150 → 149

Constat lu et vérifié : le manifeste original privé a 150 lignes, dont une entrée `./SHA256SUMS` qui le rend auto-référent. Le nouveau manifeste externe ne contient que 149 chemins Cargo/toolchain/src/tests. Son SHA-256 est exactement :

```text
b5a59c6b7b20650804778f018aff785a6328235b8bb854f2940a68c91be6cf19
```

Commande de lecture réellement exécutée par cette revue : `shasum -a 256 -c …/broker-service/source.sha256`, successivement depuis le checkout, le snapshot original `/private/tmp/ai-guard-final-canaries.kOMSpQ/snapshot` et l'archive déjà extraite `/private/tmp/ai-guard-final-canaries-proof.7zXKhV/extracted`. Résultat **149 OK / 149, exit0**, dans les trois emplacements ; aucun échec de checksum.

L'archive privée a le hash documenté `1648d46b21128b7f2dc86037229d0026a75ae4cd89cfbba824da62e0f2fafea2`. Sa liste contient 149 fichiers réguliers et 23 répertoires ; aucun lien, manifeste, AppleDouble, frontend, cache ou binaire. Le manifeste privé externe a le même hash que la copie publiée. Les copies binaires privées ont exactement les hashes documentés : agentsandbox `22a059b8f7d57437d4aa4447cfe25e5d2833980cea6fb9e3fa53cd4651c231c7`, ai-guard-provider `a0ea9da6af195d61cc588cd45aee553f199371993a9516b8cb8bb0b69b5f5ade`.

Les 149 checksums identiques entre snapshot original et checkout confortent explicitement l'absence de changement de source/test pour la correction de provenance. Ce changement ne doit ni effacer l'ancien manifeste ni être décrit comme un nouvel essai. README/provenance/VALIDATION disent bien cela. Aucune reproductibilité bit à bit du build n'est revendiquée.

### Réalité des preuves et écarts ouverts

- Le child natif vide n'est confondu ni avec un worker, ni avec un slot runtime non vide, ni avec le bootstrap public du parent. SIGSTOP/SIGCONT/même epoch/shutdown sont sa portée nommée. Le service/API authentifié A/B et les deadlines de workers après perte du CLI/broker restent NOT_RUN.
- Les trois revues sont annoncées comme lectures d'ingénierie ciblées. Les rapports service/STOP conservent leurs constats et empreintes historiques ; leurs recoupes ultérieures ferment les défauts **en source**, sans prétendre être des exécutions du reviewer. Le rapport enforcer/workspace ne s'approprie pas les tests orchestrés par Root.
- Les journaux `fail-*` restent présents : compilation de fixtures, auth absente du child direct, oracles outbox/provider et Clippy. Leur correction n'est pas requalifiée en vulnérabilité démontrée. Les fixtures runtime/autorités/numéros d'audit outbox sont nommées synthétiques, les fichiers Journal/Store/sockets sont nommés réels.
- D5/disarm, TTY, cancel ACK/retry de stop, fsync de l'export après panne et travail d'énumération incluant exclusions restent explicitement ouverts. La source d'un arrêt/claim/terminal inconnu n'est pas transformée en fence. Capacité100, caps physiques, matrice Docker/Podman/Linux, cloud et gestionnaire OS restent hors preuve.
- CURRENT_STATE, VALIDATION et le README gardent G09 FAIL et 0/12 gates fermés. Le changement n'est pas nommé production-ready, MVP ou clôture de verticale ; aucun cutover du CLI par défaut n'est revendiqué. Le rappel Daybreak Blue reste subordonné aux vrais scénarios workers.

## Deux observations éditoriales mineures

**DOC1 — statuts du plan répétés sans séparation historique assez nette.** `IMPLEMENTATION_PLAN.md:6–20` décrit le code raccordé et les lots logiciels cochés. La section basse « Raccord service actif — base c4c4d2a » conserve à `:243–246` des cases « Finaliser transport… » et « Raccorder profils… commandes IPC et CLI réelles… » non cochées, sans préciser qu'elles sont le plan initial au RED. Les travaux runtime demeurent légitimement ouverts, mais les items combinant raccord source et qualification mélangent deux états. Suggestion : dater cette liste comme historique ou distinguer raccord logiciel effectué et preuves runtime restant ouvertes. Aucun impact sur le compte40 ou un gate.

**DOC2 — liens de sources gelées non portables dans le rapport STOP publié.** `stop-recovery-review.md:14–39` utilise des liens absolus vers `/private/tmp/ai-guard-stop-recovery-frozen.zyWCZk/...` et `:26` vers `/Users/louis/Documents/ai-guard/.worktrees/broker-service/tests/...`. Les sources privées existent sur ce poste et les SHA sont fournis ; le lecteur d'un clone ne peut toutefois pas suivre ces liens. Suggestion : conserver les chemins privés comme provenance textuelle de l'original, et fournir des chemins dépôt avec mention explicite du gel/ancienne ligne, ou des extraits bornés permettant de retrouver le mécanisme. Ne pas remplacer silencieusement les liens historiques par les lignes actuelles d'un fichier modifié. Les liens principaux README→trois rapports/source/provenance/logs existent bien.

## Statuts de cette revue

PASS : compte40 dédupliqué, profils des logs, canaris finaux 3/10/5, hash du manifeste, 149 checksums dans trois emplacements, hash/liste de l'archive et hashes des copies binaires, portée limitée/gates ouverts cohérents.

FAIL bloquant de preuve : aucun constaté. DOC1/DOC2 : observations de maintenance documentaire, sans modification par cet agent.

NOT_RUN : Cargo, tests produit, bootstrap parent, moteurs, slots, service/API, fournisseurs, frontend, E2E et cloud par cette revue. Aucun résultat d'exécution produit nouveau n'est produit par la vérification de checksums.
