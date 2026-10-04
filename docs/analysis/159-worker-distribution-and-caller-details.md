# ADR-565 — видеть последовательную подготовку и распределение заданий

04.10.2026, исходный `main` `8eb409e`, WBPERF-001/027.
Исправлен анализ существующих трасс. Production engine и установленный
полигон сохранены: испытанный вариант parallel rubble support отклонён.
Нового FPS gain в этом шаге нет; 165 FPS и редкие длинные паузы открыты.

## Что мешало пониманию

Native CSV уже содержал `detail` boundaries для motion, heat и debris,
включая settled boxes/queries/candidates/cells. `summarize_tick` их
игнорировал: отчёт показывал большую родительскую фазу и несколько
долгих worker scopes, но терял подготовку и интервалы между заданиями.
Physics compaction сохранял полные per-thread totals, однако анализатор
сливал их по family. Actor scopes также теряли полные итоги по потокам.
Общий job count не показывает распределения стоимости: задания разного
веса, parent/child перекрываются, CPU counters округляются.

Теперь отчёт показывает:

- Последовательные detail intervals внутри группы от `*_begin` до
  границы phase/tick; незаписанный хвост назван явно. Группы не соединяются
  через другие фазы. При отсутствии begin group остаётся unidentified.
  Clock identity/range сохраняются; detail с чужого caller thread отклоняется.
- Mean per traced tick, occurrence p99 и число occurrences каждого detail.
  Detail уже включён в parent и не прибавляется к нему.
- В медленных ticks — clipped union записанных scopes, наибольший
  промежуток без записанного scope, покрытие caller и отдельных потоков.
  Nested scopes на одном потоке не удваивают union; разные потоки не
  складываются в elapsed wall. Unknown events остаются явно неизвестными.
- Полные validated physics totals по OS thread/stage/family и полные
  recorded actor/navigation totals по thread/family. Все группы в JSON,
  в Markdown ограниченная таблица top24. Physics summary counters
  не заменяются длительностями сохранённых longest samples.

Это покрытие записанных wall scopes, **не CPU utilization, idle или lock
wait**. Для compacted physics union — lower bound; незаписанный промежуток
может содержать отброшенные короткие задания. Parent может ждать child.
CPU/cycle samples сохраняют доступность и исходное округление, без
пересчёта cycles в ms и без суммирования вложенных families как загрузки CPU.

Новые расчёты выполняются только offline. Simulator, native callbacks,
thread pools, budgets, clocking и режимы приёмки не изменены. Phase ring
не получает выдуманных worker данных. Full CorePhases остаётся opt-in
diagnostic и не квалифицирует FPS.

## Отклонённая параллелизация опор

Замороженный кандидат распределял независимые stacked-support queries
пакетами256 при 1024+ кандидатах, с indexed collection и ordered merge.
51 physics test прошёл, включая reference comparison на 1023/1024/1537
stacked pieces, 1/2/4/8 workers, изменение опоры и позы.

Две соседние CPU пары, каждая3600 ticks, порядок before→after / after→before:

| Mean µs | Before A | After A | Before B | After B |
|---|---:|---:|---:|---:|
| Rubble | 1001,35 | 999,13 | 999,13 | 1016,21 |
| Full tick | 12447,21 | 12442,90 | 12486,73 | 12524,04 |
| Rubble max | 2758 | 2385 | 2896 | 14427 |

Gain неустойчив, второй tail хуже. Каждый tick's counters и полные final
gameplay/unit lines совпали, hash `bcc1f58c226d1721`. Причина пика не
установлена; parallel candidate и его tests полностью откатились.
Нет native FPS acceptance для отклонённого варианта. Его binary/source,
build manifest и raw measurements остаются в `out/fix565/`.

## Два последовательных native diagnostic captures

Один frozen ADR-564 binary, SHA256
`E59DEA7E2243C940BE6AFD61951657B06D8A925884538AF85F686BD30A692AC9`,
1600×900 AutoNoVsync, stock 30×30 AI vs AI, physics/navigation8/8,
compute4,120с. CorePhases+OsPressure+CpuThreads; во втором добавлен
wall-clock CameraTour. Во время captures не было наших builds/tests/edits
или тяжёлого анализа. Одинаковые исходные13 WIP, не clean checkout.

| Diagnostic | Stationary | Camera tour |
|---|---:|---:|
| Actual/traced ticks | 3600/3600 | 3584/3584 |
| Evidence valid | true | true |
| Strict realtime | true | false |
| Realtime ratio | 0,999618 | 0,995067 |
| Core per actual step, ms | 14,608 | 14,540 |
| Frame p99, ms | 30,857 | 36,329 |
| Frame max, ms | 59,499 | 89,227 |
| FPS acceptance | false | false |

Stationary hash `bcc1f58c226d1721`; camera завершилась на другом tick,
hash `30de3dd038335d2e`. Это диагностические разные workloads, не gameplay
или FPS A/B. Camera:21 segments,153 origins,0 blocked frames, longest
sampled no-tick33,092мс, streaming max18,923мс. Mouse/orbit/pixels не
проверялись. Просадка строгого темпа не скрывается и gate не расширяется.

В обоих captures наблюдаются8 route OS threads и8 translate OS threads,
166 physics thread/stage/family groups и96 actor thread/family groups.
Потоки работают; global CPU percentage сам по себе не доказывает
отсутствия параллелизации или её достаточности.

Stationary tick2543: wall47,338мс, motion33,600мс. Detail routes26,388мс,
route actor23 занял25,794мс, local_advance25,789мс. Это локализация
наблюдённой задержки в продолжении поиска, не объяснение OS preemption.
Средние details: routes2,810мс, ordered commit1,220мс, feet0,924мс;
debris translate0,677мс, wake0,611мс, candidates0,506мс.

Camera tick3553: wall63,660мс, motion38,122мс, rubble18,162мс.
Routes16,458мс содержат2,944мс recorded scope union и longest unobserved
span13,415мс. Ground10,329мс содержит1,270мс recorded union; feet8,990мс
содержит8,924мс recorded union. Отчёт теперь различает эти случаи.
Причина незаписанных промежутков и длинных scope walls без OS scheduler
stacks остаётся неизвестной. В каждом capture два50мс stall episodes;
прежние двухсекундные freezes не повторились, но не объявлены исправленными.

## Проверки и следующий приоритет

122 analyzer tests и PowerShell validity/parser suite проходят. После
отката повторно прошли50 tests финальной physics source; установленный
`bench_battle.exe` также восстановлен к frozen ADR-564 baseline.
Старый и новый анализаторы на реальном полном3600-tick capture дают
одинаковые **все исходные поля**: phases, counters, FPS/tempo gates,
alignment, task families, OS pressure, gameplay. Добавлены только детали,
thread totals и явные ограничения. Во всех ticks обоих captures групповые
thread totals согласованы с validated family totals. Исходные13 WIP и
production burst source сохранены по bytes; установленный binary прежний.

WBPERF-027/001 partial: новые отчёты показывают куда направить dispatch/
join и OS scheduler traces, но не заменяют их. WBPERF-002 — главный
архитектурный пункт: main продолжает ждать авторитетный tick. WBPERF-003/004:
route continuation tail, повторная synchronous preparation при свежих
приказах и разница между logical work и OS wall. Увеличивать pools без
этих различий и без FPS A/B недостаточно.

Raw: `out/fix565/`, два native directories с `.analysis.json/.md`,
CPU variants/counters, rejected source/build manifest, tests и
`compatibility.log`. Ничего из raw не публикуется как production code.
