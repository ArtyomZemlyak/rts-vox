# ADR-544 — позднее покрытие диагностической трассы рендера

## Подтверждённая помеха анализу

После ADR-543 быстрый detail run исчерпал лимит 100000 записей: 5806 spans
потеряны, 447 source rows отсутствуют. Это не отсутствие работы в позднем
бою: recorder прекращал добавлять записи после заполнения. Самые тяжёлые
поздние prepare/extract уже нельзя привязать к source ID и worker CPU.

Вторая помеха: список тяжёлых render frames сортировался по всему schedule.
Кадр с дорогим graph/present мог вытеснить из короткой таблицы другой кадр
с более дорогим prepare вне gas. Газовые actual calls уже имеют отдельные
рейтинги; теперь residual prepare тоже ранжируется независимо.

## Фикс

Диагностическая schema 4 рассчитывает ёмкость до измерения:
`min(1000000, max(100000, (ceil(seconds)+1)*400*13))`. 400 FPS — планирование
запаса, не ограничение игрового FPS и не обещание достигнутой частоты;
13 — максимум текущих coarse/gas scopes на кадр. Один кадр добавлен для
терминальной передачи. Для 120 с ёмкость 629200, для 20 с — 109200.
Сотни быстрых FPS или длинный soak всё ещё могут исчерпать жёсткий потолок.

Никакого роста без предела, семплирования объектов или дискового I/O во время
кадра. Vec резервируется заранее; число retained records и dropped остаются
явными. Metadata сохраняет `render_span_bytes`: limit×size даёт резерв
элементов, не RSS процесса или полную стоимость профайлера. Обычная игра
по-прежнему не устанавливает TimelinePlugin и не резервирует этот буфер.

`--profile-render-spans 1..=1000000` / `-RenderSpanLimit N` задают явный
потолок. Ключ требует render diagnostic mode. Автоматическая обёртка не
передаёт новый CLI-ключ: замороженные schema-1/2/3 binaries по-прежнему
запускаются со своими прежними контрактами. Новый exe сам считает тот же
план; явное переопределение требует новой сборки.

PowerShell и offline analyzer проверяют schema/actual budget/counts/размер
записи. Для schema 4 лимит сверяется с manifest; drops добавляют
`render_timeline_truncated`, отклоняя comparison и `--require-valid`,
с сохранением raw CSV/покрытия. Сравнение разных schema/лимитов/размера
записи отклоняется. Форматы 1/2/3 остаются читаемыми с прежним лимитом;
их historical evidence не переименовывается в полное покрытие.

В JSON/Markdown видны `capture_not_truncated`, actual limit, резерв,
last source row и прежние complete/missing/open. Отсутствие overflow не
равно записи последней main row: терминальные open/unhanded остаются явными.
Таблица prepare вне gas получает самостоятельный рейтинг; nested wall
не складывается, residual не называется idle/ожиданием или установленной
причиной. Scheduler/function probes и частота симуляции прежние.

## Следующие границы понимания

Этот фикс закрывает потерю позднего хвоста стандартного 120-секундного
прогона, а не attribution остальных Bevy jobs. Проверены pinned исходники
Bevy 0.19.1: per-system spans требуют feature `trace`; `Schedule::systems`
даёт inventory, не время вызовов. Замена систем через
`SystemSchedule::systems_mut` прямо запрещена контрактом API как UB.
Поэтому не добавлены обходные замены или exclusive fences между prepare
jobs, которые изменили бы их параллельность и исказили измерение.

Открыты WBPERF-001/027: фактические Bevy jobs вне gas, OS stacks/waits и
GPU source IDs. WBPERF-002 — независимый sim/render и последовательный
critical path — остаётся главным архитектурным ограничением. Улучшения
CPU microbenchmark не подменяют нативный FPS; низкая общая загрузка CPU
сама по себе не доказывает наличие безопасно параллельной работы. 165 FPS
не достигнуты и этим изменением не заявляются.

## Проверка и нативные данные

130 library / 101 blast / 85 Python / 32 PowerShell = **348 проверок**
прошли, четыре manual tests остаются ignored. После механической замены
max/min на clamp повторены девять timeline tests. Release build/clippy
проходят без новых предупреждений. Восемь raw datasets ADR-543 читаются
новым analyzer; старый dropped capture явно показывает
`capture_not_truncated=false` с прежним schema-3 контрактом.

Нативные запуски последовательны, без builds/tests/правок/видео во время
измерения. Заморожен `out/fix544/after.exe`, SHA256
`fd68982cee703f9fc31b5f602f6d99f9f987d5b972b263324c877d587ded78c3`.
Он содержит ту же отдельно подготовленную работу ADR-523, не чистый Git
snapshot; SourceAtLaunch не доказывает build source. RTX 5090/Vulkan,
1600×900, pools 8/8, stock tuning/обычная картинка. Manifests, CPU samples,
main/render CSV, logs и offline reports сохранены.

| Run | Main rows | Complete source rows | Retained / dropped | Open | Missing source rows | Offline gate |
|---|---:|---:|---:|---:|---:|---|
| `out/fix544/battle-120` | 7808 | 7807 | 101491 / 0 | 0 | 1 | pass |
| `out/fix544/actions-20` | 6887 | 6885 | 89510 / 0 | 2 | 1 | pass |
| `out/fix544/forced-overflow`, 5 с, limit 10 | 913 | 0 | 10 / 11838 | 0 | 912 | expected rejection |

В длинном бою source row 7806 записана целиком; отсутствует только последняя
main row 7807, ещё не handed off. Это на 115 complete source rows больше,
чем вместил бы прежний cap в **этом же** потоке из 13 scopes/frame.
Размер Span в сборке 336 bytes; резерв на 120 с — 201,62 MiB элементов,
не RSS. Actions сохраняет поколения 1…4, один terminal partial render
и одну ещё не handed-off main row; это не overflow.

Forced overflow: оба инструмента возвращают
`render_timeline_truncated`; native manifest invalid, offline
`--require-valid` exit **2**. Raw evidence и dropped count сохранены,
residual не вычислен по обрезанному дереву.

Бой делает 3601 шаг за ~120 с, ratio 0,999605×, hash `abb9829fad382c19`.
Средний observed frame 15,3766 мс / 65,034 FPS, p99 36,0457 / max 71,0337
мс, 5104 кадров сверх 6,061 мс. Это diagnostic run, **не FPS A/B** и не
приёмка 165 FPS. Сравнение с ADR-543 по FPS здесь не заявляется.
Core mean 15,5349 мс/step; actual gas extract max 0,4814 мс.

Самостоятельный prepare-рейтинг: row 5017 / tick 1451, parent 32,1266 мс,
actual gas 0,0457 мс, residual 32,0809 мс; graph 3,3338 мс.
Driver CPU counter 0,000 мс округлён, cycles 3621030 ненулевые; это не
доказательство 32 мс idle/lock/GPU wait. Other workers остаются вне coarse
driver CPU. Без per-system/OS instrumentation конкретную причину этой
паузы назначать нельзя. Теперь она и поздние кадры сохраняются в полном
потоке и отдельном рейтинге для следующего этапа.
