# Revue ciblée P0 — indirection privée du profil de start

1 novembre 2026. Revue indépendante en lecture seule du code de `/Users/louis/Documents/ai-guard/.worktrees/docker-launch-profile`. Aucun source modifié, Cargo/test/build, moteur, processus applicatif, réseau, `.env` ou secret consulté/exécuté. Seule cette note privée est créée.

Source revue : `src/runtime.rs`, SHA256 **`adddd62b03c85b0869caf29d9c8ec2cee82c224059c9a2fc42e0d237aad348ad`**, vérifié au début et à la fin de la lecture. Base comparée : `sources-after.tar.gz` de `/private/tmp/ai-guard-launch-profile-p0-qualification-20261101T030800Z`, runtime.rs SHA256 `aed653fb7af242f1b45619c0c22094ab17c29a9599e1933b59a9b856b8d758ea`.

AGENTS.md, RTK.md, ARCHITECTURE_CONTRACT.md, CODING_RULES.md, PRODUCTION_READINESS.md, TEST_STRATEGY.md et `/tmp/ai-guard-launch-profile-contract.md` lus. Les deux emplacements attendus de `ai-guard-security-review` sont absents dans ce worktree ; aucun chargement de ce skill projet n'est revendiqué. Pas de nouvelle revue globale du lot.

## Constats sur le delta

**Aucun défaut bloquant trouvé dans ce delta.** Le diff de runtime.rs contre l'archive comporte exactement cinq hunks : deux champs privés deviennent `Option<Box<DockerLaunchProfileBindingV1>>`, le constructeur privé applique `map(Box::new)`, la comparaison emprunte les valeurs, et deux constructions du test sont adaptées.

- **Représentation/API** — `PrearmedStartControl::launch_profile` (runtime.rs:33) retire le binding possédé inline de la charge des enums. `PrearmedStartControl::new`, `PrearmedStartDirective` et `OwnedStartDirective` gardent leurs signatures/variantes. Les champs sont privés ; aucune sérialisation, canonicalisation ou représentation disque n'est concernée. La taille effective et la disparition du lint restent à confirmer par Clippy, sans mesure de taille inventée.
- **Égalité complète** — `expected.as_deref() == prepared.as_ref()` (runtime.rs:63) compare deux `Option<&DockerLaunchProfileBindingV1>`. Le `PartialEq/Eq` dérivé de ce binding (runtime/launch_profile.rs:36–51) couvre tous ses champs ; ce n'est ni une comparaison d'adresses ni une comparaison du seul SHA de contenu. Les quatre combinaisons None/Some gardent leur sens.
- **Garde consommée** — le wrapper possède l'unique `Box<dyn BeforeSpawnGuard>` et son `check_before_spawn(self: Box<Self>, ...)` compare avant d'appeler une seule fois `self.inner.check_before_spawn(target)` (runtime.rs:61–66). Un désaccord retourne avant cet appel ; le permis n'est ni cloné, ni rejoué, ni déballé pour permettre un second usage.
- **Ordre de préparation** — la nouvelle allocation du binding a lieu dans `with_launch_profile` (runtime.rs:43–51), appelé par l'enforcer depuis la registration vérifiée (enforcer/client.rs:883–892). Le wrapper est alloué avant le dernier contrôle (runtime.rs:1161). Préparation de commande, revalidation du profil et clone des métadonnées restent dans `prearmed_command` (runtime/managed.rs:545–579), avant la garde finale. Le profil retenu reste possédé par `_retained_profile` pendant le transport.
- **Couture finale** — runtime.rs:1183–1187 conserve l'appel consommant de garde suivi de `command.spawn()`. Aucun await, IO, préparation de commande ou callback supplémentaire n'est inséré par ce correctif. La comparaison empruntée n'alloue pas. Sur le chemin accepté, les closures de diagnostic ne sont pas appelées ; la destruction des métadonnées privées n'ajoute aucune IO applicative.

## Vérification pertinente et limites

La capture initiale prouve `fmt-check.rc=0` et **`clippy.rc=101`**, avec deux erreurs `large_enum_variant` explicitement liées à la charge `PrearmedStartControl` d'au moins 256 octets. Elle ne prouve pas Clippy sur la source corrigée.

La régression directement pertinente est `runtime::capture_error_tests::profile_guard_compares_complete_binding_before_consuming_existing_guard` (runtime.rs:1947) : None/None et profils identiques consomment une fois ; None/Some, Some/None et parent différent ne consomment pas l'inner guard. Ses deux adaptations ne changent pas les assertions. Pour qualifier ce delta : fmt, Clippy strict all-targets et cette fonction exacte sur le SHA ci-dessus ; exécution confiée à Luna, **NOT_RUN par ce reviewer**. Les 12 régressions antérieures du lot ne sont pas transférées comme nouveau PASS à ce SHA.

Cette revue établit la cohérence du correctif de représentation et de son ordre local. Elle ne qualifie ni Docker réel, ni D5, ni G09, ni un autre gate ; leurs écarts restent ouverts.
