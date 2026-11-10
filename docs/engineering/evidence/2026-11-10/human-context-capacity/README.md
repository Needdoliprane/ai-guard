# D8 — validation ciblée de la capacité des contextes humains

## Résultat logiciel final — 10 novembre

Sur le gel composé2336
`38ce0d5f9489696dc9205872c24b2bcfb6f08dc21a48ee3f3add42b626411b46`,
le build lib `--no-run` passe, puis **10 parents D8 et11 contrôles existants
passent**, et `cargo check --frozen --offline --all-targets` termine RC0 en
40,30s. Les six fichiers Rust intégrés correspondent exactement au manifeste
owned D8 `cffd81952345345eec89163a724ba8ba6c7a760c5bb5d8856d5770e3ede957f5`.
Les revues indépendantes couvrent code, raccords d'initialisation, correction
d'oracle et huit chemins intégrés, guides EN/FR compris. Aucun défaut bloquant
restant relevé dans ces périmètres.

Commandes supplémentaires réellement exécutées sur ce gel :

```text
cargo test --frozen --offline --lib --no-run
cargo test --frozen --offline --lib api::human::context_fairness_tests::d8_real_identity_pressure_keeps_b_and_mixed_human_context_available -- --exact --nocapture --test-threads=1
cargo test --frozen --offline --lib api::human_state::tests:: -- --nocapture --test-threads=1
cargo test --frozen --offline --lib api::read_execution_tests::source_read_admission_rejects_before_opening_local_authentication_transactions -- --exact --nocapture --test-threads=1
cargo check --frozen --offline --all-targets
```

Avec les trois commandes D8 historiques ci-dessous, les sélections sont2+1+6+1
pour D8,10+1 pour les contrôles, soit21 fonctions uniques PASS. Le producteur
HTTP complet conserve les64 identités locales A,32 memberships et2 modes ; B
et l'humain A+B restent disponibles selon leur part, sans contexte B préalloué.

Raws hôte directs SHA-256, dans l'ordre build, D8 contrôles/court/registre/
producteur, contrôles historiques et check :

- `06307617faef18e82c4ea789c19adaa88cb40238e2dea72236041df93e7d4d25` ;
- `0a6d6a1f92049d84c9a034fbdaf2346401fc3598798d027e2dbf45ec34dbc55a` ;
- `5bdbf20218f624b21360cf409750f3ecd918effdaf78abc1be947112ed8ef387` ;
- `651ce884e892d5829e52c346437929da3d39f92255706350a85d577821e7bccf` ;
- `19fd82a214c40c9bfa5e1d2033850f19144e6a2ef7e08fb826e04267daed155c` ;
- `8b15a733d8e2692bd2ef02b1a9bb3a46c61ea0fa40f2ae4d2eb05a222a66e89b` ;
- `88c58017073928d2e34a38a5f7848c55c4eeb90dcf38590f0ad4d11495243f50` ;
- `fcf2cf183a4c686dcecd0303f6dc7da0c06b7cd7f55bc5c5bacb218c0250ccd1`.

QA5c185… : UID1000/caps0/NNP/seccomp, réseau none, rootfsRO, CPU6/RAM9Gio/
swap0/PIDs128/scratch3Gio. Source effective recalculée avant run exactement
égale au manifeste attendu ; Rust/Cargo1.99.1, lock et cache publics vérifiés.
La première archive macOS produisait des AppleDouble non listés : extraction
rejetée avant build et remplacée par un paquet GNU des seuls fichiers attendus.
La première composition R2 échoue en compilation sur un actor manquant dans
la fixture ingress ; R3 ajoute uniquement cet acteur synthétique existant,
sans modifier D8. Échecs et manifestes précédents conservés.

La preuve valide le correctif local du registre humain ; elle ne réserve pas
la capacité Store/CPU/ingress/OIDC et ne qualifie ni IdP, runtime, disponibilité
physique, charge100 ni produit complet. Les autres WIP du checkout ne reçoivent
aucun PASS implicite. La QA reste ouverte pour des lots indépendants, avec ses
preuves hors worktree ; fermeture et cycle des worktrees suivis séparément.

## Essais préparatoires conservés

10 novembre2026 : le candidat était dans un worktree séparé. Les gels privés
ci-dessous conservent leurs résultats historiques ; les commandes s'appliquent
aux sources nommées, pas implicitement à un autre checkout.
Aucun worker métier, fournisseur, IdP réel, modèle ou cluster utilisé ; Docker
sert uniquement de sandbox aux compilations et tests logiciels ci-dessous.

## Sources et revues

Le RED registre avait atteint l'assertion4 096A/0refusalsA/B Capacity, source
composée2414 `c67e2903f81785d10c114a86a586c9f0a168afeacc9c3263ccd38b5a720d604c`.
Raw `cdf7ca280097e91353c8f2eb7019dc7ad1a4d120238e73e5ba6412aded856761`.
C'est une preuve du registre avec bindings synthétiques, pas du producteur HTTP.

FIX R1 :2334 fichiers, manifeste
`1b91a29e5d63adaf790ed0398ac32ee85cecf4257e2bf640c5818254ab447472`.
Revue indépendante des six sources et revue Root des trois raccords
d'initialisation. R2 conserve tout le produit et corrige une attente de test :
patch `abc2a129f808f8de1852a875e125dc17c38526dbe2e150327aed32fd568bf26a` ;
fichier `e188c6f21a941281a838765244055234ca704d514e2a278d3efb9448e50f3688` ;
full2334 `3bf60e762a09a2f4f09c83976e2deec195db1b68defc6062ee5728c78e98e5e1`.
Second regard indépendant confirme le contrat préexistant active=false →
401/identity_revoked/authenticate ; pas de changement de l'authentification.

## Commandes et résultats réellement observés

```text
cargo test --frozen --offline --lib api::human::context_fairness_tests::d8_registry_pressure_from_a_preserves_b_configured_share -- --exact --nocapture --test-threads=1
cargo test --frozen --offline --lib api::human_state::context_fairness_tests:: -- --nocapture --test-threads=1
cargo test --frozen --offline --lib api::human::context_fairness_controls:: -- --nocapture --test-threads=1
```

R1 :1 PASS, puis6/6 PASS, puis1 PASS/1 FAIL (neuf uniques exécutés).
Raws SHA-256 respectifs :

- `95bc6e81ce5ba95f7a85a3e4d9fdcf76dde75d057220a82f631c36d0197b7996` ;
- `2e5c3faa5334cbadc22950843f6503e0f82a44bc7c02afdb45124e37290b1860` ;
- `1d366a025abc3cec8c190dbfd5c93b581cbcde0bbec724140dfa212ecb0583df`.

Le dernier FAIL est l'attente `credential_invalid` au lieu du réel
`identity_revoked`. Le test exige toujours401 et authenticate. Les avertissements
de compilation sont conservés, sans Clippy réputé exécuté.

R2 : même module contrôles tenté sur les nouveaux octets vérifiés, poignée61740
terminée137. Aucune sortie Cargo récupérable, sélection ou RC interne confirmé.
Statut `0f8389ebd673f1f2e4358dfcd635a001dac9846e368537963203793065306181`.
La tentative est non concluante ; aucun parent R2 n'est déclaré PASS.

**NOT_RUN** : producteur Router complet
`d8_real_identity_pressure_keeps_b_and_mixed_human_context_available`, canaris
de cette révision et check toutes cibles. Ne pas remplacer ce producteur par
le seul registre ni transférer un PASS historique entre gels complets.

## Environnement et conservation

QA isolée : image épinglée2115d432…, Rust/Cargo1.99.1, dépendances verrouillées
hors réseau, UID1000/caps0/NNP/seccomp, rootfsRO, réseau none, CPU4/RAM8Gio/
swap0/PIDs128, scratch3Gio/65536inodes et une seule source/cible privée. Le
contrôle de source est passé avant chaque GO. Aucun accès au home, socket
moteur, credential réel ou preuve privée depuis la QA.

R1 post-check2334 RC0 et export source réel
`b354c45fcd981c2a413ef9499308796d2006f6171b2ce6b44bae43866dbe6271`.
Aucune fixture temporaire D8 restante trouvée après R1 : aucune archive prétendue.
R2 conserve source d'entrée et manifeste, mais pas le raw ni un export après
essai ; le fichier vide est nommé comme placeholder. Le gel hôte ne remplace
pas une attestation de fin d'exécution.

La seule QA attribuée est arrêtée, PID0/exit137/OOMfalse, watcher terminalRC0,
puis retirée normalement. L'échéance prévue01:55UTC n'est pas l'arrêt constaté
02:10:42UTC ; sa cause n'est pas établie. Les prochains essais doivent séparer
la compilation des tests et capturer les sorties directement sur l'hôte.
Les preuves détaillées restent hors worktree dans la racine privée
`qa-human-context-d8-20261110-r1/`, distinctes des rapports d'audit originaux.

## Acceptation à l'issue des essais préparatoires

Il restait à obtenir les dix parents du FIX, dont le producteur HTTP, les canaris pertinents et
la compilation requise sur une source identifiée, traiter les revues, intégrer
les guides EN/FR, publier puis fermer les worktrees après inventaire. La part
volatile du registre ne réserve pas CPU/Store/ingress/OIDC et ne prouve ni
latence ni disponibilité physique. Les résultats logiciels sont désormais
consignés en tête ; aucun gate global n'est fermé par ce lot.
