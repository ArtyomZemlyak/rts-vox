# 641 — review двух ui_motion test-only cases Kimi

Reference: `out/kimi-ui-motion-review-20261116/out/ui-motion-time-tests.patch`.
SHA256 проверен:
`9c63c6b3c7477d661aacfb4745f7f9f6c95aa30c9848de9d8b3707dff3136ce1`.
Прочитан ui-motion-time-tests-evidence.json. Старые 12/12 за 0,126 с
не используются как результат текущего кода.

## Review актуального покрытия

| Предложенный случай | Текущий main | Решение |
|---|---|---|
| Zero-delta и exact endpoint | contract_tests::start_middle_exact_end_and_zero_delta_use_real_time_for_every_motion_kind: все 8 kinds, реальный Time, start/mid/end−1/end, нулевые кадры и скрытие декора | Не дублировать |
| Open→Close / Hover→Unhover | contract_tests::real_panel_visibility_and_hover_reverse_without_stale_parallel_effects | Существующий тест сохранён |
| Чужой owner сохраняет effect и age | contract_tests::retrigger_restarts_only_the_matching_event_and_preserves_other_owners для Select→Select | Существующий тест сохранён |
| Чужой owner во время именно reversal двух семейств | Сочетание отсутствовало | Перенесён только этот test-only случай |

Добавлен один `ui_motion::tests::reversing_one_owner_preserves_other_effects`.
Он адаптирован к текущему Duration; старые f32 age/tick из patch не возвращены.
Оба owner имеют эффект одного семейства, чтобы проверить адресность замены
Open→Close и Hover→Unhover. Проверяются число эффектов, новый kind/нулевой age
целевого owner и неизменные kind/age другого owner.

Runtime, SIM, input, существующие тесты и contract_tests не менялись.
Полная свежая проверка contract_tests не заявлена: review покрытия выполнен
по коду, запуск ограничен запрошенным ui_motion::tests::.

## Приёмка

Текущий `--filter ui_motion::tests::` прошёл: **13 passed, 0 failed,
0 ignored, 0 skipped**, выполнение 0,065 с, cached development build 10,779 с.
Использован существующий test-fast, sim-test/offline, debug_assertions=false,
affinity до 8 CPU, Rayon 2.

Source SHA256:
`40aa8dc0e548ad05962d25e97624e1fd7b2f6536f5e6542914712ae0673a2c3c`.
Receipt: `out/ui-motion-review641/focused/test-index.json`.

Аудит подтвердил единственную вставку нового теста: остальной файл, runtime,
старые тесты и contract_tests сохранены. Подстановка исходного digest этого
файла только в памяти воспроизводит исходный общий compile-source hash:
других изменений compile inputs в этом пакете нет.

Review, выборочный перенос и focused-проверка — done; внутри пакета remaining
нет. Full 446/native/release/FPS не запускались, старые допуски не выдаются
за новые. Процесс завершился штатно, активных процессов не оставлено.

Доказательства, точный diff и ledger: `out/ui-motion-review641/final-evidence.json`,
`rebased.patch` и `ledger.json` там же.
