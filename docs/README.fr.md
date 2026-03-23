<p align="center">
  <img src="../assets/wizard.png" alt="wshm" width="600"/>
</p>

<p align="center">
  <a href="../README.md">English</a> •
  Français •
  <a href="README.es.md">Español</a> •
  <a href="README.de.md">Deutsch</a> •
  <a href="README.ja.md">日本語</a> •
  <a href="README.zh.md">中文</a> •
  <a href="README.ko.md">한국어</a> •
  <a href="README.pt.md">Português</a>
</p>

---

# wshm — Agent IA pour la maintenance de repos GitHub

**Your repo's wish is my command.**

wshm est un outil CLI + GitHub Action qui agit comme un agent autonome de maintenance de repos. Il tourne sur chaque nouvel issue ou PR, et sur un schedule pour la detection de conflits.

## Fonctionnalites

- **Triage automatique** — Classification, labelling et priorisation des issues par IA
- **Analyse de PRs** — Resume, evaluation de risque, checklist de review
- **Auto-fix** — Generation de PRs draft pour les bugs simples (seuil de confiance)
- **Merge queue** — Scoring et classement des PRs, auto-merge au-dessus du seuil
- **Resolution de conflits** — Detection et resolution automatique (jamais de force-push)
- **Review inline** — Commentaires de review ligne par ligne par IA
- **Auto-assignation** — Attribution ponderee des maintainers aux issues et PRs
- **Blacklist de labels** — Empecher l'application de certains labels
- **Retriage periodique** — Re-evaluation des triages obsoletes selon un intervalle
- **Notifications** — Resume quotidien des priorites vers Discord, Slack, Teams ou webhook
- **Dashboard et rapports** — Dashboard HTML et rapports markdown/PDF
- **Entierement personnalisable** — Templates pour chaque commentaire et branding

## Comment ca marche

Un seul binaire. Vos cles API. Vos donnees. Zero infrastructure.

```
GitHub API → Sync incremental → SQLite local → IA → Actions (label, comment, PR, merge)
```

Trois modes : CLI, GitHub Action, ou daemon persistant avec webhooks/polling.

## Securite

- **Dry-run par defaut** — `--apply` requis pour agir
- **Seuil de confiance** — jamais d'action autonome en dessous de 0.85
- **Jamais de force-push** — resolution par nouveaux commits
- **Idempotent** — re-executer = meme resultat
- **Tokens securises** — toujours depuis les variables d'environnement

## Acces anticipe

> **wshm est actuellement en beta privee.**
>
> Pour tester wshm sur vos repositories :
>
> **[contact@rtk-ai.app](mailto:contact@rtk-ai.app)**
>
> Nous cherchons des early adopters. Mainteneurs open-source et petites equipes bienvenus.

---

<p align="center">
  <sub>Construit en Rust. Zero infra. Un seul binaire.</sub><br>
  <sub>&copy; 2026 <a href="https://rtk-ai.app">rtk-ai</a></sub>
</p>
