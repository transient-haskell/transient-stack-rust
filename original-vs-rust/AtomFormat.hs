{-# LANGUAGE ScopedTypeVariables, OverloadedStrings #-}
import Transient.Internals
import Transient.Loggable
import Transient.Parse
import qualified Data.ByteString.Lazy.Char8 as BL
import qualified Data.ByteString.Char8 as BC
import qualified Data.Text as T
import Data.ByteString.Builder
import Control.Monad.IO.Class

ser :: Loggable a => String -> a -> IO ()
ser label x = putStrLn (label ++ " => " ++ show (BL.unpack (toLazyByteString (serialize x))))

-- what `deserialize` reads from a text (up to the first undoubled `/`), and what is left
des :: forall a. (Loggable a, Show a) => String -> a -> IO ()
des input _ = do
  (r, _) <- runTransient $ withParseString (BL.pack input) $ do
              (x :: a) <- deserialize
              rest <- giveParseString
              return (x, BL.unpack rest)
  putStrLn ("  read " ++ show input ++ " => " ++ show r)

main = do
  putStrLn "-- integers"
  ser "Int 0" (0 :: Int); ser "Int -5" (-5 :: Int); ser "Int max" (maxBound :: Int); ser "Integer grande" (123456789012345678901234567890 :: Integer)
  putStrLn "-- Double"
  mapM_ (\(l,d) -> ser l (d :: Double)) [("0.1",0.1),("1.0",1.0),("100.0",100),("0.01",0.01),("1e7",1e7),("9999999.0",9999999),("123456.789",123456.789),("1.5e10",1.5e10),("1e-3",1e-3),("-2.5",-2.5),("0.0",0),("-0.0",-0.0),("NaN",0/0),("Infinity",1/0),("-Infinity",-1/0),("5e-324",5e-324),("3.14159",3.14159),("0.1+0.2",0.1+0.2)]
  putStrLn "-- Float"
  mapM_ (\(l,d) -> ser l (d :: Float)) [("3.14",3.14),("1e-2",0.01),("16777216",16777216)]
  putStrLn "-- Bool, unit, Char"
  ser "True" True; ser "False" False; ser "()" ()
  mapM_ (\(l,c) -> ser l (c :: Char)) [("'a'",'a'),("'\\n'",'\n'),("'\\''",'\''),("'\\\\'",'\\'),("'/'",'/'),("'e acute'",'é'),("'euro'",'€'),("'\"'",'"')]
  putStrLn "-- String ([Char])"
  mapM_ (\(l,s) -> ser l (s :: String)) [("empty",""),("hello","hello"),("with slash","a/b"),("double slashes","a//b"),("quotes","a\"b"),("space",  "a b"),("unicode","ñandú"),("euro","€"),("newline","a\nb"),("ends in slash","a/")]
  putStrLn "-- Text"
  mapM_ (\(l,s) -> ser l (s :: T.Text)) [("hello","hello"),("with slash","a/b"),("unicode","ñandú"),("euro","€")]
  putStrLn "-- ByteString"
  ser "bs with slash" (BL.pack "a/b")
  putStrLn "-- reading"
  des "42/rest" (0 :: Int)
  des "-5/x" (0 :: Int)
  des "3.5/x" (0 :: Double)
  des "3/x" (0 :: Double)
  des "1.0e-2/x" (0 :: Double)
  des "t/x" True
  des "u/x" ()
  des "'x'/y" ('a')
  des "hello//world/rest" ("" :: String)
  des "a b/rest" ("" :: String)
  des "/x" ("" :: String)
