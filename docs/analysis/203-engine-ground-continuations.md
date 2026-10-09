# Грунтовые продолжения волн без повторной подготовки корпусов

Дата: 07.10.2026. Общий `Explosion::step_contacts` после [пакета 202](202-engine-wave-lazy-geometry.md) ещё строил bounds всей армии и выполнял partition очереди при каждом посещении отложенного кратера. [План общего движка](200-engine-performance-plan.md), EP-04b, разделяет сокращение работы внутри вызова и изменение правил admission. Здесь сохраняется прежний физический контракт.

## Проверка оставшейся работы по сохранённым курсорам

У текущих волн тела посещаются локальными клетками один раз на первую subcell крупного контакта. Когда cursor прошёл все unit actors и body_cursor равен нулю, далее в этом контакте остаётся только грунт. Проверяется каждый pending contact: курсор головы берётся из существующих полей, остальных — из сохранённой `contact_resumes`. Непосещённый новый контакт и частично обработанное тело запрещают быстрый путь.

У старых point-sampled волн тела могут встречаться в последующих subcells. Для них условие строже: при наличии юнитов только последний ground cursor доказывает отсутствие дальнейших body samples. Пустой roster не требует геометрии тела ни в одной модели. Грунтовые контакты по-прежнему проверяют актуальный материал, накопленный surface work, энергию и физическую вместимость.

Когда вся очередь доказанно содержит только оставшийся грунт, общий kernel обслуживает её без `HitFrames`, bounds, массива next_actor и body/ground partition. Пропускается только подготовка и выбор уже пройденных unit actors. Все ground subcells, contact visits, отказ/уступка очереди, оплаченные префиксы и conditional edits исполняются прежним порядком. Проверка заново выполняется после нормативного heap advance и rotation на каждом вызове; новый контакт с телом возвращает обычный путь.

Переход между ticks не кешируется. Новых авторитетных полей и версии сохранения нет: решение выводится из существующих saved cursors. `wave_ground_only` считает вызовы и visits только для диагностики, сбрасывается при restore и не влияет на hash/решения. Bench печатает эти counts в `--damage-trace`; schema tick CSV остаётся прежней. Оптимизация работает и у обычного range, и в общем battle kernel, без условий по карте, стороне или ID орудия.

## Проверки эквивалентности и завершения

Дифференциальный oracle сохраняет прежние eager geometry и FAIR rules. Сравниваются snapshots мира/материи/юнитов, serialized pressure frontier, contact/resume queues, surface changes и физические ground/body admission outcomes на каждом шаге. Новые пробы требуют реально ожидающий грунт, возврат вместимости, завершение всех subcells и подтверждение принятых conditional edits, а не только опустошение pressure queue.

Покрыты крупные ячейки 1/2/4, local-cell контакты и legacy points, нулевая/единичная/малая квоты, сохранение между отказами, изменение текущей позы после посещённого frozen body prefix, поздний новый контакт с телом и body priority при полном пуле. Отдельная legacy проба требует реального урона телу в поздней subcell: ранний ground cursor не может её пропустить. Существующие scalar, capacity, worker determinism, weapon, save и golden проверки сохраняются.

Физическая pressure queue и world edit queue завершаются независимо. Тест продолжает confirmation после завершения pressure: принятые claims должны передать всю материю в debris и снять ownership, без удаления очереди. Это уточнение проверки завершения, не изменение фаз или вместимости игры.

## Измерения большого боя

Последовательный headless контроль: общий world battle, AI vs AI, 3600 ticks, physics/navigation workers=8/8, release/offline/locked. Принятая frozen сборка 202, новая сборка и повтор новой запускались с одинаковыми `--damage-trace --tick-csv`; другие сборки/тесты во время замеров не выполнялись. Replay проверяется отдельно и не входит в сравнение времени.

| Показатель | Принятый пакет 202 | Новый путь | Повтор нового пути |
|---|---:|---:|---:|
| Средний tick, мс | 14,985 | 13,464 | 13,504 |
| p99 tick, мс | 22,111 | 20,334 | 20,416 |
| max tick, мс | 36,291 | 35,130 | 35,774 |
| Wave step на tick, мс | 2,794 | 1,322 | 1,322 |
| Tick последних 150 ticks, мс | 17,467 | 15,478 | 15,565 |
| Wave step последних 150 ticks, мс | 2,938 | 1,052 | 1,052 |
| Полные bounds preparations | 39 076 | 8704 | 8704 |
| Подготовленные actor maps | 5907 | 5907 | 5907 |

Средний tick дешевле на **9,9–10,1%**, wave step — на **52,7%** относительно принятого пакета 202. Motion остаётся около 5,16–5,21 мс, debris — 2,61–2,64 мс, felling — 0,194–0,195 мс на tick. Выигрыш соответствует изменённой волновой фазе; сравнение не включает новую AI policy или уменьшение физической работы.

Новый путь исключает 30 372 повторные bounds preparations и partitions (77,7%). Он обслужил **10 645 408** нормативных contact visits в 30 372 calls. Этим retries не удалены: каждый по-прежнему проверяет живой грунт/ownership/capacity, платит прежнюю квоту и сохраняет продолжение при отказе. Оба новых прогона дают одинаковые диагностические counts.

Во всех трёх прогонах hash `5dd0c782d2b2f557`, alive [26,21], базы [100,100], полные массивы shots/damage и peak_rounds=30 совпали. Сверены все 25 нетайминговых CSV fields на каждом из 3600 ticks, включая очереди, submitted work, motion, fragments/flying, wave callbacks, felling arrivals/retire/release/deferrals и capacity scans. Все физические `damage_*` windows совпали: ground/body admission, тепло, wake, kinetic и machine-gun continuations. Из сравнения исключены только диагностические geometry и новый ground-only counter.

На tick 3600 felling по-прежнему 2 915 882, collapse_queued — 2 013 153. Ускорение обслуживания не доказывает прогресс этих очередей и не закрывает нехватку физической вместимости.

## Квалификация и паспорт

Полный `cargo test --release --locked --offline -p sim-core -- --test-threads=4` прошёл: **731 passed, 0 failed, 6 ignored** (684 lib, 3 bench, 44 integration). Три новые differential пробы прошли вместе с существующей проверкой реальной стрельбы обеих армий. `cargo check --locked --offline -p render --bin blast` прошёл. Это результаты sim-core и сборки render; они не объявляют весь workspace зелёным: четыре ранее отмеченных отказа architecture suite из отчёта 201 остаются отдельным долгом.

Контрольный executable пакета 202: SHA256 `b17df251f1908074d6a8690c6dd555bed9e06af43d461faf4e161757684d9be6`. Новый frozen `bench-after.exe`: `9dc9d9c58936a084396b6e9f89796142ffaed3e383e6b9edb2851948f8dd1c1b`. Паспорт `after-inputs.json` содержит 356 входов. С принятого пакета 202 изменены только `blast.rs`, `range_matter.rs`, `range_wave_resume_tests.rs` и вывод диагностического counter в `bin/bench_battle.rs`; scenario/data, AI policy и зависимости прежние.

Отдельный `--replay --snapshot-tick 3300 --ticks 3600` прошёл: snapshot 134 186 763 bytes, hash восстановленного и исходного боя на tick 3600 — `5dd0c782d2b2f557`. После всех тестов и замеров повторно сверены все 356 manifest inputs, основной/frozen executable и контрольный binary 202. `qualification.json` фиксирует результат. `git diff --check` проходит.

Raw материалы: `out/engine-fixes-20261007/ground-continuations/{before,after,repeat}.{log,csv}`, `compare.py`, `comparison.json`, `all-tests.log`, `render-check.log`, `replay.log`, `after-inputs.json`, `qualification.json`. `out` ignored; ключевые результаты сохранены в этом отчёте, наличие raw в другом checkout не предполагается. Время относится к этому headless workload на одной машине; native FPS не измерен.

## Открытые части плана

Сокращается подготовка уже посещённых тел; сами отказы admission не пропускаются. Частичные body contacts по-прежнему требуют обычной геометрии. Нехватка working/parked storage и длительные continuations остаются физической проблемой: дополнительный cap, удаление старой волны и отказ от урона не являются этим исправлением. Дренаж collapse/felling, stationary AI под большей нагрузкой и освобождение физической вместимости EP-04b/c остаются открытыми. Общий вход команд, read model и owner thread EP-02/03 не меняются этим пакетом. Headless выигрыш не является замером native FPS.
