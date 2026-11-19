# 640 — construction: штатные timed upgrades вместо fixture override

Reference: `out/kimi-ui-current-check-20261115/out/construction-current-consolidated.patch`.
SHA256 проверен:
`49a8390e205e955e91e6d706b27f1e9f7f60a8c1b55acc846c081a49ac38e187`.
Прочитан consolidated-evidence.json. Старые isolated 15/15, 3/3 и catalogue
1/1 не используются как текущая приёмка.

## Review и перенос

Текущий fixture действительно устанавливал `timed_upgrades=false`.
Эквивалента объединённому пакету не было. Изменён только
`crates/render/src/bin/blast/construction/tests.rs`:

- Удалён override, добавлено требование штатного timed_upgrades=true.
- Три настоящие T1-электростанции добавлены в конец roster: прежние IDs
  сохранены. Ёмкость fixture и stock `1 << 36` оставлены без изменения.
- Два теста проходят T2/T3 и T2/T3/T4 через обычные `upgrade_kil` и `sim.step`.
  Свой союзный танк получает Hold, чтобы бой не закончился раньше производства.
- Общий test-helper проверяет реальную ожидающую очередь, время, ненулевое
  потребление, подключённую генерацию и частично оплаченное улучшение.
  Промежуточный snapshot восстанавливается, оба экземпляра делают один шаг,
  hashes сравниваются; тест продолжается на восстановленном экземпляре.
  Завершение требует следующего tier, пустой очереди, большей материи корпуса
  и сбалансированного ledger. Фазы, таймеры и платежи не подменяются.
- Stale-card проверяет настоящий Greben Engineer → Factory switch.
- Tooltip сравнивается с текущей production quote, включая массу в проекции.

Reference hunk с `queue_row=Flex` не перенесён: в текущем UI строка скрыта,
поскольку очередь принадлежит верхнему островку. Сохранены более новые
assertions для этой строки, name-only captions, энергии tooltip, реального
числа страниц и actual hover/capture preview. Остальные 14 тел тестов
побитно совпадают после нормализации перевода строк:
`out/construction-review640/preserved-tests.json`.

SIM, balance, RON, production runtime и UI runtime не менялись.

## Проверки

Focused validation прошла через существующий scripts/test-fast.py,
sim-test/offline, debug_assertions=false, GPU-free, affinity до 8 CPU,
Rayon 2. Оба прогона использовали один исходник и один test binary:

`97b2c592e1a5ba9476a8816aa3d5a94fa8e08670f7c1204b9cc711b7f9672900`.

| Набор | Passed / failed / ignored | Время тестов | Receipt в out/construction-review640 |
|---|---|---|---|
| Три изменённых теста | 3 / 0 / 0 | 96,730 с | changed-focused/test-index.json |
| Все локальные construction-тесты | 16 / 0 / 0 | 175,595 с | construction-local/test-index.json |

Первый build занял 87,97 с, второй прогон использовал проверенный кэш.
Три изменённых теста входят в эти 16, не считаются повторно как отдельные
19 проверок. Свежий hover/capture preview и остальные неизменённые проверки
также входят в локальные 16.

Полный обход всех stock-миров
`construction::tests::all_stock_scenarios_offer_only_their_live_allied_builders_recipes`
исключён явным --skip и записан в receipt. Его тело не изменено; старый timeout
не переименован в pass. Он остаётся за пределами текущего focused-допуска.

Аудит compile inputs заменяет только digest текущего tests.rs исходным digest
в памяти и воспроизводит исходный общий source hash. Другие compile inputs
не изменены: `out/construction-review640/compile-input-audit.json`.

## Done / remaining

Review/rebase, штатный fixture, реальные timed T2/T3/T4, питание/оплата,
промежуточный restore, сохранность более новых UI assertions и focused
construction — done. Внутри этого test-only пакета remaining нет.

Полные stock-миры, общая UI/native/FPS приёмка и доставка не заявлены.
Release и «Полигон» не менялись. Проверки завершены, контроллер закрыт после
получения успешных receipts; активных процессов не оставлено.

Доказательства и текущий ledger: `out/construction-review640/final-evidence.json`,
`out/construction-review640/ledger.json`. Точный перенос — `rebased.patch` там же.
