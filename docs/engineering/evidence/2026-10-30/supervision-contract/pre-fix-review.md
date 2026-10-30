# Contrats de supervision — revue indépendante préparatoire

30 octobre 2026. Lecture uniquement du nouveau module et du raccordement ; aucun test, service, moteur ou fournisseur exécuté, aucun fichier source/doc du dépôt édité. Store/IPC/armement réel sont absents et ne sont pas qualifiés par cette revue.

Gels vérifiés : `src/control/supervision.rs` `ee928524b1ddee65f65b35b583c8b3502dfb0ff74db081d83587e6ed39b0a697` ; `src/control.rs` `122e0d27d4bd184c11e6c4af18c942089a4c9d15ae07f3f31604808d37d82458` ; contrat v2 `/tmp/ai-guard-broker-control-contract-v2.md` `89da46a488395079b35673474cdbd8ed47a854cbc6ace440bcccc1bd2bdc508f`.

**Verdict : deux défauts P2 de validation confirmés par lecture, déjà signalés par root et soumis au RED Luna. Correction et preuve ciblée nécessaires avant acceptation du module. Aucun autre défaut concret établi dans ce périmètre.** Ni effet moteur ni contournement d'une API livrée n'est prétendu ici.

## SC-01 — P2 : un boot UUID nul satisfait la validation

`supervision.rs:272–286` appelle le validateur UUID historique à 275. `control.rs:387` accepte tout UUID de forme canonique, dont `00000000-0000-0000-0000-000000000000`. Aucun `is_nil` spécifique ne suit.

Reproduction minimale à conserver en test : construire un `HostClockSampleV1` version1/méthode valide, boot nul, compteur `"0"`, UTC absente ; `validate()` et `continuous_ns()` retournent actuellement Ok. Deux tels samples peuvent aussi satisfaire l'égalité de boot d'un arm/clock_check. Le contrat natif exige un boot strict non nul ; une identité manquante ne doit pas être représentée comme un boot vérifié partagé entre redémarrages.

Correction : rejeter nil dans le nouveau type d'horloge, sans changer le validateur UUID global ni les contrats/hashes historiques. Régression : nil refuse ; UUID canonique non nul accepte ; absence de fallback de boot lors des comparaisons.

## SC-02 — P2 : `validate_for_arm` accepte une autre révision de claim

`supervision.rs:572–576` vérifie forme du claim, reçu exact, sujet Worker et fenêtre d'horloge, mais jamais `claim.expected_revision == arm.expected_revision`. Les deux valeurs sont seulement contraintes à être positives.

Reproduction minimale : arm Worker valide à révision R, reçu dont input_hash correspond exactement à cet arm, clock_check dans la fenêtre ; claim du même worker/attempt/reçu avec expected_revision R+1. `validate_for_arm` retourne actuellement Ok. Changer cette seule révision ne doit pas conserver la validation du lien arm/claim ; l'armement ne change pas lui-même la révision worker.

Correction : comparer les révisions dans le validateur relationnel. Cela s'ajoute au futur CAS transactionnel contre la révision courante du Store ; cela ne le remplace pas. Régression : claim R accepte ; R+1 et R-1 positifs refusent avec tous les autres champs identiques.

## Contrôles satisfaisants et portée

- `helper_execution_policy_v1` dérive les bindings parent, manifeste, helper/nom/volume/image/autorité et quatre finalités fermées ; tar/cwd fixes, network none, preset2, CPU2000/RAM1073741824/PID256/120s/restartNo, aucun credential. Le parent TTY3 conserve son propre hash tandis que la policy helper reçoit preset2 et le domaine distinct `ai-guard:helper-execution-policy:v1`.
- `ArmV1::validate_for_parent` recalcule cette policy, compare les champs d'inspection et son hash ; il ne réutilise pas le hash de commande métier pour tar. Import/ImportVerification exigent Preparing, quota retenu et aucun create/start ; FinalScan/Export exigent terminal sans quota. La preuve de réussite de l'import/permission reste explicitement une consultation durable du futur Store.
- Compteurs u128 décimaux canoniques, longueur/overflow contrôlés ; deadline par addition checked, clock_check même méthode/boot et intervalle fermé à gauche, ouvert à droite. UTC reste diagnostic. Le défaut nil est isolé ci-dessus.
- Reçu d'arm lié à opération/worker/attempt/hash exact ; claim helper lié au helper du sujet et à la fenêtre d'horloge. Namespace/audit hash/reçu de permission ne peuvent être authentifiés par un DTO seul : leur résolution contre le Store demeure obligatoire, sans les transformer en grants publics.
- Les observations gardent les informations temporelles insuffisantes pour une future classification Unknown ; leur forme seule ne prouve ni terminal crédible ni allocation. Pas de finding artificiel pour l'absence de transactions dans ce module de types.
- Diff de `src/control.rs` contre `/tmp/ai-guard-provider-prv-fixed.vWMHOh/src/control.rs` : seuls `mod supervision` et réexports ajoutés. Types, fonctions de validation et domaines de hash legacy inchangés ; aucune migration ou activation implicite introduite.
- Les `validate`/`input_hash` sont des primitives de forme/liaison, pas une validation indépendante du moteur ou de l'horloge hôte. Les DTO publics ne portent pas de valeur credential/Env, ni de source privée à ouvrir. Les contenus shape-only ne doivent pas devenir des autorisations dans les prochains lots.

Statuts : revue **effectuée**, SC-01/02 **OUVERTS** au gel ci-dessus, RED/GREEN **non attribués tant que leurs logs ne sont pas recoupés**, module préparatoire **À CORRIGER** ; supervision complète **NON LIVRÉE/NON QUALIFIÉE**. Les deux corrections restent dans ce module et ne requièrent pas de réexécuter les lots API/HTTP ou fournisseurs.
