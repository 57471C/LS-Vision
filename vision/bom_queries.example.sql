-- Override this file with vision/bom_queries.sql. Separate the kit header and parts with a blank line.
-- A line that is only "-- ---" is also a split.
-- Kit header
SELECT des AS KitName
FROM IR
WHERE ic = @kitIC;

-- Parts
SELECT
    b.CC AS IC,
    ir.DES AS DES,
    -b.CQ AS QTY
FROM bm AS b
INNER JOIN IR AS ir ON ir.IC = b.CC
WHERE b.MC = @kitIC
  AND b.CC <> b.MC
  AND b.CC NOT LIKE '3%'
  AND b.CC NOT LIKE '2P%'
ORDER BY b.bmid;
