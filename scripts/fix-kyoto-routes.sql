INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, duration_min, notes, start_time) VALUES
-- Day 1: Arrival
('kyoto-2026', 'kyoto_2026', 1, 0, '台北市北投區關渡', '嘟嘟房桃園機場貨運1站', 'driving', 45, NULL, '05:00'),
('kyoto-2026', 'kyoto_2026', 1, 1, '嘟嘟房桃園機場貨運1站', '桃園國際機場第一航廈', 'transit', 15, '免費接駁車', '05:45'),
('kyoto-2026', 'kyoto_2026', 1, 2, '關西國際機場 KIX', '京都駅', 'transit', 75, 'JR Haruka 特急（含套裝）', '15:15'),
('kyoto-2026', 'kyoto_2026', 1, 3, '京都駅', 'hotel', 'walking', 3, NULL, '16:30'),
-- Day 2: Kinkakuji + Nishiki Market + Pontocho
('kyoto-2026', 'kyoto_2026', 2, 0, 'hotel', '京都駅', 'walking', 3, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 2, 1, '京都駅', '北大路駅', 'transit', 15, '地鐵烏丸線', NULL),
('kyoto-2026', 'kyoto_2026', 2, 2, '北大路バスターミナル', '金閣寺道バス停', 'transit', 15, '101或102號公車', NULL),
('kyoto-2026', 'kyoto_2026', 2, 3, '金閣寺', '四条河原町', 'transit', 40, '公車至四條河原町', NULL),
('kyoto-2026', 'kyoto_2026', 2, 4, '四条河原町', '錦市場', 'walking', 5, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 2, 5, '錦市場', '先斗町', 'walking', 5, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 2, 6, '先斗町', '四条駅', 'walking', 5, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 2, 7, '四条駅', '京都駅', 'transit', 5, '地鐵烏丸線', NULL),
('kyoto-2026', 'kyoto_2026', 2, 8, '京都駅', 'hotel', 'walking', 3, NULL, NULL),
-- Day 3: Hozugawa River Boat + Arashiyama
('kyoto-2026', 'kyoto_2026', 3, 0, 'hotel', '京都駅', 'walking', 3, NULL, '10:20'),
('kyoto-2026', 'kyoto_2026', 3, 1, '京都駅', '亀岡駅', 'transit', 25, 'JR山陰線', '10:50'),
('kyoto-2026', 'kyoto_2026', 3, 2, '亀岡駅', '保津川遊船乘船場', 'walking', 10, NULL, '11:15'),
('kyoto-2026', 'kyoto_2026', 3, 3, '保津川遊船乘船場', '嵐山渡月橋', 'transit', 120, '保津川遊船', '11:30'),
('kyoto-2026', 'kyoto_2026', 3, 4, '嵐山渡月橋', '嵐山竹林小徑', 'walking', 10, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 3, 5, '嵐山竹林小徑', '天龍寺', 'walking', 5, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 3, 6, '天龍寺', '嵐山渡月橋', 'walking', 10, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 3, 7, '嵯峨嵐山駅', '京都駅', 'transit', 15, 'JR嵯峨野線', NULL),
('kyoto-2026', 'kyoto_2026', 3, 8, '京都駅', 'hotel', 'walking', 3, NULL, NULL),
-- Day 4: Kimono + Higashiyama + Gion
('kyoto-2026', 'kyoto_2026', 4, 0, 'hotel', '京都駅', 'walking', 3, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 4, 1, '京都駅', '夢館 五条店', 'transit', 10, '地鐵烏丸線至五條，或計程車', NULL),
('kyoto-2026', 'kyoto_2026', 4, 2, '夢館 五条店', '二寧坂', 'walking', 20, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 4, 3, '二寧坂', '三年坂', 'walking', 3, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 4, 4, '三年坂', '八坂之塔', 'walking', 5, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 4, 5, '八坂之塔', '祇園', 'walking', 10, NULL, NULL),
('kyoto-2026', 'kyoto_2026', 4, 6, '祇園', '京都駅', 'transit', 20, '公車或計程車', NULL),
('kyoto-2026', 'kyoto_2026', 4, 7, '京都駅', 'hotel', 'walking', 3, NULL, NULL),
-- Day 5: Departure
('kyoto-2026', 'kyoto_2026', 5, 0, 'hotel', '京都駅', 'walking', 3, NULL, '09:00'),
('kyoto-2026', 'kyoto_2026', 5, 1, '京都駅', '關西國際機場 KIX', 'transit', 75, 'JR Haruka 特急（含套裝）', '09:30'),
('kyoto-2026', 'kyoto_2026', 5, 2, '桃園國際機場第一航廈', '台北市北投區關渡', 'driving', 60, '接駁車至嘟嘟房，再開車回家', '16:30');
