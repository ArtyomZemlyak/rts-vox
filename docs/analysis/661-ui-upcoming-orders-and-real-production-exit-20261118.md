# 661 — Следующие заказы, сборка/оплата и настоящее завершение производства

2026-11-18. Продолжение UI после660. База `64951ca2936aca1b96b5f0c87d99c30ca6d95657d2e0a15ee5dee53682812088`;
проверенный source `a6e6775d82a4609e5d21c833006096b28f01633b9952c2fb5c41b2f64b7dd563`; WGSL `cfc028d29b03091a2e033d4b1a144853f9b685b917f82cf5b7e9c7bfe2d073ba` неизменён.
Изменён только `construction/production_hud.rs`. Все предыдущие WIP сохранены.
AGENTS, CLAUDE, текущий ui-optical-work и матрица ui-style-system учтены;
палитра/все9референсов и визуальный результат660 используются как принятая база.

## Изменение

В постоянном верхнем острове появились два pooled места следующих заказов.
Объединяются только соседние одинаковые заказы; после двух групп показан
точный остаток `+N`, порядок не меняется. Это данные, не ложные кнопки отмены.
При смене количества/групп высота строки остаётся32px. Компактный остров
960×540 занимает269×126px; раскрытие одного производителя111px.
Пространственная проекция учитывает реальный ComputedNode верхней очереди.

Раскрытие показывает отдельно настоящую сборку и оплату из единственного
`production_progress` запроса, а для стройки — доставленную/необходимую материю.
`готово` допустимо только при выполнении обоих условий. Причина блокировки
выхода не выдумывается. Янтарные выбор/прогресс, cyan проекции и нижние якоря
сохранены. Нового каталога/очереди внизу, зависимостей или SIM-правок нет.

## Проверки

| Область | Результат |
|---|---|
| Focused production HUD + ui_motion |25/25 passed,skip0|
| Расширенный текущий UI |72/72 passed,1явный skip ниже|
| Architecture/docs |15/15 passed,skip0|
| Реальное производство/холодный restore |240ежетиковых W1/W2 hash совпадений|
| Native completion |12.005606s realtime1×,359кадров,ticks111..344|
| Native ввод/motion |11s realtime1×,317кадров,16/16valid input,errors0|
| Три разрешения/интенсивность |960×540/100,1920×1080/35,3840×2160/0 и540details|

Явный skip: `construction::tests::all_stock_scenarios_offer_only_their_live_allied_builders_recipes`.
Это прежняя дорогая full-world проверка, не новый pass. Настоящие timed upgrades
в остальных construction tests сохранены. Полные миры для этого UI-only diff
не запускались повторно. Повторение focused внутри expanded не считается
дополнительными уникальными тестами.

Положительный сценарий использует существующую construction fixture,
штатные timed rules,три PowerPlantT1 и явный тестовый запас материи.
Не сокращены время/цена/условия производства. На tick111 сборка110/200,
оплата12106575/22011954. `production_exit(1)=(16,200)` наблюдается после
шага201: создан Tank,очередь `[Tank,Skat,Tank,Tank,Artillery]` становится
`[Skat,Tank,Tank,Artillery]`. К351 новый танк `[2132,15,900]` освободил
ворота (`factory_exiting_unit=None`), ledger balanced.
Холодный restore W2 совпал с W1 по state_hash на каждом из240шагов,
финальный hash`c5c760474f337e6d`. Native clip воспроизводит тот же холодный
снимок: видно завершение,смену текста/следующих заказов и выезд. Нативный
hash из изображения не присваивается: authoritative SIM proof и визуальная
запись приложены отдельно.

В native motion проверены реальные hit rectangles/блокировка,hover,
раскрытие,выбор,меню. Миникарта/Stop стабильны,object/recipe protected overlaps0,
minimum panel font13physicalpx. Этот отдельный motion clip produced/completed0;
он не заменяет настоящий completion clip. Оба видео декодированы FFmpeg,
тайминг сохранён по wall timestamps без ускорения. Визуальный осмотр проведён
по companion previews; full PNG использован только для точечного crop очереди.

## Боевой background A/B

Одинаковые мир/входы/seed,workers2/2,affinity8,cap200,profile-core на обеих
сторонах; общие ticks60..306,
workload signatures и InputHashes совпадают. Четыре20s development прогона,
1080 before→after,4K after→before; кодирование завершено до измеряемых окон.

В каждом прогоне73объекта,включая здания. На общем окне пики:18361фрагмент,
22живые волны,325380pending валки,31moving/20searching. Нулевой pending edits
на отдельных render-маркерах не означает отсутствие разрушения:в логе/CSV
идут реальные выстрелы/урон/удалённый грунт. Конечные shots/damage при разных
стоп-тиках сохранены отдельно и не приравниваются к общему окну.

|Прогон|FPS|p50 ms|p95 ms|p99 ms|max ms|SIM×|max debt ticks|dropped ticks|private/working GiB|
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| perf-1080-before | 16.181 | 61.62 | 99.20 | 119.04 | 124.65 | 0.724 | 19.0 | 74.4 | 18.54/15.86 |
| perf-1080-after | 16.147 | 61.94 | 98.77 | 119.34 | 123.43 | 0.720 | 19.0 | 76.3 | 18.52/15.84 |
| perf-4k-before | 16.071 | 62.32 | 100.23 | 120.92 | 124.50 | 0.721 | 19.0 | 75.4 | 18.92/15.93 |
| perf-4k-after | 15.820 | 62.12 | 100.33 | 121.35 | 123.49 | 0.718 | 19.0 | 77.0 | 18.87/15.93 |

Память — process private/working set, выборка 1 Гц; это не VRAM.
В этой паре FPS изменился на −0,21% в 1080p и −1,56% в 4K;
SIM работает на 0,718–0,724× с пропущенными тиками, общий допуск не закрыт.

Это диагностика фонового полноценного боя с73объектами,а не ordinary FPS,
release-допуск или доказательство5000юнитов. Нагрузка и фазы доступны в
`out/ui661/battle-summary.json`. Пауза,запись completion и статичные кадры
не включены в боевые измерения. По одному pair на разрешение недостаточно
для статистического общего no-regression допуска; результаты сохранены целиком.

## Done / remaining

|Статус|Работа|
|---|---|
|done|661:pooled upcoming groups,build/payment,реальное завершение/выезд,restore,UI/input/visual/A-B evidence|
|remaining627|Обычный foreground FPS остаётся открытым по выбору пользователя;нужна общая immutable интеграционная приёмка/release/launcher|
|remaining scale|128/1000/5000 сSIM debt/выполненными приказами иworkers1/2/4/8 — отдельная уже поставленная работа|
|remaining mechanics|Существующие623/628/625остатки из общей queue не объявлены закрытыми этим UI-only diff|

Development binary: `out/ui661/blast-ui661.exe`,SHA`f040b52fc8520bcacd6586701ae613bed46dc20c1fd4c23a8926544bd2169444`.
Release/«Полигон» не переключались: общий627допуск ещё не закрыт.

## Evidence

- `out/ui661/source-audit.json`, `reviewed-current-diff.patch`, `build-dev.json`.
- `out/ui661/checks-focused-completion/test-index.json`, `checks-expanded/test-index.json`, `checks-docs/test-index.json`.
- `out/ui661/production-v2/proof.json`, `run.log`,cold `near-completion.bin`.
- `out/ui661/after-production-540-100/video/realtime-1x.mp4`,frame timestamps/capture.json.
- `out/ui661/after-motion-540-100/video/realtime-1x.mp4`,input.tsv,geometry.tsv.
- `out/ui661/visual-review-data.json`, `queue-completion.preview.jpg` и4static previews.
- `out/ui661/battle-summary.json`,4native execution/manifests/CSV/core/memory logs.
