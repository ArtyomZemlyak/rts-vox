# 639 — review test-only пакета Kimi

Уточнение643: --skip уже перенесён,но отдельный ранний отказ для пустого
`--skip ""` отсутствует. Это конкретный remaining tooling-пункта; при его
проходе перенести только этот guard и focused test из replacement01 Antigravity.
Ниже сохранены прежние результаты,которые не покрывают этот отдельный случай.

Проверены три файла reference patch:
`out/kimi-test-fast-skip/test-fast-improvements.patch`, SHA256
`1360c29b990e48c2ff16de1d59d0cc73202588fe81a6e4400b6de409c533794e`,
и `out/kimi-test-fast-skip/evidence.json`. Старые 194 Python, fx 28+1 и
construction inventory 15+1 не использованы как текущие результаты.

## Перенос

| Файл | Текущий результат review |
|---|---|
| AGENTS.md | Добавлен краткий маршрут movement-quick → supports / walking-blasts и требование явного учёта skip |
| scripts/test-fast.py | Перенесён отсутствовавший --skip; существующий разбор should-panic сохранён |
| scripts/test_test_fast.py | Добавлены две focused-проверки skip и should-panic |

`--skip NAME` повторяемый; подстроки применяются только после обычного
положительного выбора. JSON сохраняет нормализованные `skip_filters`, точные
`skipped`, оставшиеся `selected` и `selected_count` по каждой цели. Unknown
skip завершает runner ошибкой; пустой остаток цели запрещён. Старые guards
положительных фильтров, счётчиков, provenance и pure-pass reuse сохранены.

Одно уточнение относительно patch: запись skipped/selected выполняется до
ошибки пустого остатка. Поэтому даже ошибочный all-skipped отчёт содержит
точные исключённые имена. На этом пути libtest не запускается. Тест также
покрывает повтор одного skip-фильтра и отсутствие исключённого имени в команде.

Разбор точного ` - should panic` уже реализован текущим `completed_tests`.
Его regex не заменён эквивалентным. Новая проверка подтверждает ok/FAILED
и отказ для постороннего суффикса; FAILED не становится успешным результатом.

## Текущие проверки

| Проверка | Фактический результат |
|---|---|
| Python до переноса | 192/192 |
| Python после переноса | 194/194 |
| Реальный fx с явным skip | 31 passed, 1 skipped, 0 failed/ignored |
| Тот же исключённый fx без skip | 1 passed, 0 skipped/failed/ignored |
| Focused navigation schema без skip | 2 passed, 0 skipped/failed/ignored |

В текущем fx 32 теста, поэтому число отличается от старого Kimi 28+1.
Все 32 реально выполнены между двумя прогонами. Skip использован только для
проверки самого инструмента, не для сокрытия красного. Unknown/all-skipped —
ожидаемые отрицательные сценарии Python; полный набор завершился OK.

Вызовы идут через существующий test-fast и его прежний MSVC-адаптер,
sim-test/offline, ограниченные CPU, без второго runner. `--no-build` для
последующих focused-вызовов проверяет текущие source/environment/binary.
Game-source SHA256 остался
`6908f0f7f3b6907798c3bb555b8dc95164d1e2748055db97528fabfa2e214498`.
Runner имеет новый отдельный hash в каждом receipt.

Construction inventory здесь не повторялся: выбран быстрый focused-набор
navigation; чужие 15+1 не засчитаны. Full-world/native/FPS, release и доставка
для этого tooling-пакета не запускались и не квалифицировались.

## Done / remaining

Review, перенос только отсутствующего, быстрые проверки и отчёт — done.
Внутри этого tooling-пакета remaining нет. Он не меняет состояние отдельных
игровых задач и производительной приёмки. Активных процессов после проверок
не оставлено.

Доказательства и точный diff: `out/tooling-review639/final-evidence.json`,
`implementation.patch`, `ledger.json`; Python-логи `python-before.log` и
`python-current.log` сохранены отдельно.
