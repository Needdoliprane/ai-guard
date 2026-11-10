# D8 — validation ciblée de la capacité des contextes humains

10 novembre2026. Candidat dans un worktree séparé, non intégré à cette date.
Les gels privés sont identifiés par leur manifeste ; les commandes ci-dessous
s'appliquent à ces sources, pas implicitement à ce commit documentaire.
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

## Acceptation encore ouverte

Obtenir les dix parents du FIX, dont le producteur HTTP, les canaris pertinents et
la compilation requise sur une source identifiée, traiter les revues, intégrer
les guides EN/FR, publier puis fermer les worktrees après inventaire. La part
volatile du registre ne réserve pas CPU/Store/ingress/OIDC et ne prouve ni
latence ni disponibilité physique. Aucun gate global n'est fermé par ce lot.
