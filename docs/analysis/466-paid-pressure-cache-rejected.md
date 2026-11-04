# 466 — Cached nuclear paid pressure: отклонён по суммарной цене

3 ноября 2026. Артефакты: `out/optimization-443/{before,after}466.*`, не новый ADR.

## Гипотеза

Nuclear read-ahead уже вычислял `soil_contact_pressure` для generic soil voxels,
чтобы убрать silent contacts. Ordered apply затем повторно вычислял то же значение
через `soil_load`. Прототип сохранил adjusted pressure в payload, выровненный с
`solid_mats`. Terraced и read-ahead fallback получали `u32::MAX` sentinel и прежний
расчёт; ordinary waves не создавали pressure buffer.

Contact order, energy, claims, deferred resume, ledger, snapshot и hash не менялись.
При review добавлено обязательное sentinel-выравнивание для материалов, дочитанных
после cache miss, чтобы индекс не ссылался на pressure предыдущей клетки.

## Проверки

- 19 contact-read тестов прошли.
- Полный nuclear episode, 181 тик: все hash совпали с ADR-827.
- `removed=9 505 559`, итоговый hash `ae3798a582873796`, ledger сбалансирован.

## Производительность

| Показатель | ADR-827 | Cached pressure | Изменение |
|---|---:|---:|---:|
| Mean nuclear tick | 17,173 мс | 17,496 мс | +1,89% |
| Худший тик | 32,197 мс | 33,210 мс | +3,15% |
| Суммарный `physics_wave_claims` | 541,177 мс | 528,698 мс | −2,30% |
| Суммарный `read_ground` | 173,136 мс | 186,977 мс | +7,99% |
| Суммарный `read_coarse` | 165,568 мс | 180,013 мс | +8,73% |
| Суммарный `read_pack` | 32,041 мс | 36,454 мс | +13,77% |

Apply действительно стал дешевле, но pressure writes, дополнительный buffer и pack
стоили больше. Это отрицательный суммарный результат, поэтому ordinary replay и ABBA
не оправданы.

## Решение

Прототип и изменения двух test callers полностью откачены. `blast.rs` восстановлен к
18 insertions/4 deletions ADR-827 относительно Git baseline. Суммарные принятые
−10,82% ordinary mean tick и −14,17% позднего окна не изменились.

Вывод: повторное использование уже вычисленного pressure не окупается при текущем
объёме payload. Следующий P0-C кандидат не должен добавлять per-voxel буфер без
доказанного превосходства над стоимостью его записи и упаковки.
