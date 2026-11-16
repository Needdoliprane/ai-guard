# 16 novembre — D1 pré-démarrage : reproduction logicielle sur le code courant

Le fingerprint `control-monitor-prestart-failure-unmonitored-start:src/runner/control_monitor.rs:run` du run `ai-guard-run-2-2026-11-09` est reproduit sur une fixture locale avec le vrai CLI, le gateway et SQLite. Le moteur est un programme Rust synthétique : aucun conteneur Docker/Podman ni fournisseur IA n'est exécuté. Aucun nouveau verdict de sévérité ni qualification runtime n'en découle.

## Sources et prérequis

- Code produit équivalent au commit public `a3981316d5a0ff1678dee7d943971fa845991e84` ; seuls les documents d'ingénierie et le nouveau test distinguent les gels.
- Archive de test : `c60b0cb088491416bcfb4ca09936a4c81d99020b737145004f6794cbae24cc53`, 2 926 fichiers ; index `caa69f997e9ccad6cb1820576435fb367113eb7e78cf5996545b8c3be9d2f61c`.
- Nouveau test formaté : `cbc612d1664ced3384ac7f75dcc9d2e51eb8813310453c643d96844ffd9c51ea`. Il attend déjà le comportement sûr et reste privé en attendant le correctif ; ce n'est pas un test PASS livré sur cette branche.
- Compilation réelle CLI + intégration RC0, artefacts `fresh=false`, découverte des deux FQN actifs et liste ignorée vide.
- Profil natif macOS ARM64 : Rust 1.99.1, opt0/debug0, assertions et overflow checks actifs, codegen256, incremental désactivé. Aucun agrandissement de pile, de TTL ou d'epoch.

Les deux scénarios exécutent le CLI enfant plutôt que d'imbriquer `runner::run` dans le futur du test. Le RC134 historique avant oracle reste conservé ; son frame fautif n'est pas identifié et aucun correctif de pile n'est revendiqué.

## Résultats constatés

| Cas exact, une invocation | Résultat | Oracle réellement atteint |
|---|---|---|
| `canary_cli_owner_enforces_durable_stop_without_prestart_fault` | PASS RC0, 8,36 s | Running, `command.started` et époque du moniteur, puis arrêt durable, Terminal, quota libéré, kill exact et audit sain. |
| `prestart_read_failure_refuses_cli_start_and_retains_audited_owner` | FAIL RC101, 15,45 s : RED du déclencheur D1 | La première lecture retourne 429 pendant que Created est retenu et après rétention du CID. Une inspection du moniteur est observée ; ensuite un premier start métier est émis. |

La ligne brute du second cas est :

```text
D1_ORACLE fault=true setup=true monitor_inspected=true owner_exited=false starts=1 exact_kill=false audit_valid=true
```

La première assertion fautée exige zéro start et en observe un. Les assertions communes de setup, fermeture CLI/serveur, captures bornées, absence de credential dans la sortie, intégrité de l'audit et projet original intact précèdent cette assertion et sont passées. Les observations sont prises avant la fermeture synthétique du moteur de fixture.

**Limite :** les assertions suivantes de rétention, d'état InDoubt, de quota et de claim n'ont pas été atteintes. `owner_exited=false` et `exact_kill=false` décrivent la fenêtre d'observation ; ni settlement complet ni confinement réel ne sont déduits. Les assertions de fermeture du CLI et du serveur passent ; le TMPDIR privé est vide.

## Reçus conservés

| Pièce | SHA256 |
|---|---|
| Build et découverte | `71a40dc45082a9b9213433d2064d11a2a6bbf8803016c380f9f5a631273eddd9` |
| CLI réel | `2638ab9c7474041589d097baf0e3e407d719df130ec09f438534af168e94e04f` |
| Binaire d'intégration | `bc0e77110f695eb3f35e295010c988150af706fe1b51c490791317934d486cb0` |
| Canari | `c8cf72d5308f5f42d17bcd3c0efcb2a8d5a8cb59a11999fb78dd789ec1207cc0` |
| Vérification Root du canari | `b140e5c65b118122ccbbfdd9f86ae16348f7a71bb4f0f024bbe37aacf43a452c` |
| Faute baseline | `579bca8cf2b24d2ada7ab4cc6d579f8b0ebde6d3a1a7e6c99225c556e59457be` |
| stdout de la faute | `0a3f6aaa0ea5c192f21ed33befa1a7143a95bfedbfb415d188f42be8ad2dd98a` |
| Vérification Root de la faute | `369bb8cb42b6a2512ae84165b0e6d3750f082bfd21314efe8be96442a07721ab` |

Les commandes réelles sont un build ciblé `cargo test --test audit_run2_d1_prestart --no-run --locked --offline --message-format=json -j1`, puis chaque FQN sous `--exact --test-threads=1 --show-output`, avec une limite externe de 240 s. Les budgets SETUP30/ORACLE10/DRAIN10 et worker120 sont inchangés. Les trois incidents de préparation antérieurs, sans invocation Cargo ou test, restent dans les pièces privées ; aucun n'est converti en PASS comportemental.

## Correction et portée restante

La correction est en cours, avec contrôle adjacent au premier effet, possession/révocation et drainage réels ; deux flags runner seuls ne démontrent pas la fermeture de toutes les courses. Elle devra repasser canari et régression, puis être relue indépendamment. Aucun correctif produit D1 n'est publié par cette note. Aucun fingerprint ni gate de production n'est clos. Voir le [registre run-2](../../../AUDIT_RUN_2_20261110.md).
