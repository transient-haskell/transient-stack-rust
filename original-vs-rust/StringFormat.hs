{-# LANGUAGE ScopedTypeVariables, OverloadedStrings #-}
-- Checks the format of strings, Text and Char after the fix of Loggable.hs:
-- writes, reads what was written (round trip), and also reads the old (raw) format
import Transient.Internals
import Transient.Loggable
import Transient.Parse
import qualified Data.ByteString.Lazy.Char8 as BL
import qualified Data.Text as T
import Data.ByteString.Builder
import Control.Monad

ser :: Loggable a => a -> String
ser x = BL.unpack (toLazyByteString (serialize x))

-- what `deserialize` reads from a text and what is left
des :: forall a. (Loggable a) => String -> IO (Maybe (a, String))
des input = do
  (r, _) <- runTransient $ withParseString (BL.pack input) $ do
              (x :: a) <- deserialize
              rest <- giveParseString
              return (x, BL.unpack rest)
  return r

-- writes `x` followed by a separator and a rest, reads it and checks it is the same value and that the rest is left
roundTrip :: forall a. (Loggable a, Eq a, Show a) => String -> a -> IO ()
roundTrip label x = do
  let text = ser x
  r <- des (text ++ "/rest") :: IO (Maybe (a, String))
  putStrLn $ (if fmap fst r == Just x && fmap snd r == Just "/rest" then "ok   " else "FAIL") ++ " " ++ label ++ "  " ++ show text

main = do
  putStrLn "-- String: what is written"
  mapM_ (\s -> putStrLn ("  " ++ show s ++ " => " ++ show (ser (s :: String))))
    ["", "[]", "hello", "a/b", "a//b", "a\"b", "a\nb", "nandu \241 \8364", "a b", "x:y;z"]
  putStrLn "-- Text: what is written"
  mapM_ (\s -> putStrLn ("  " ++ show s ++ " => " ++ show (ser (s :: T.Text))))
    ["", "hello", "a/b", "nandu \241 \8364"]
  putStrLn "-- Char"
  mapM_ (\c -> putStrLn ("  " ++ show c ++ " => " ++ show (ser (c :: Char)))) ['a', '/', '\n', '\233']
  putStrLn "-- lists"
  putStrLn ("  [String]: " ++ show (ser (["a", "b/c", "", "d\"e"] :: [String])))
  putStrLn ("  [Int]:    " ++ ser ([1, 2, 3] :: [Int]))
  putStrLn ("  [] :: [Int]:    " ++ ser ([] :: [Int]))
  putStrLn "-- round trip"
  forM_ ["", "[]", "hello", "a/b", "a//b", "a/", "/a", "///", "a\"b", "a\\b", "a\nb\tc", "nandu \241 \8364", "\119070", "a b", "x:y;z"] $ \s ->
    roundTrip "String" (s :: String)
  forM_ ["", "hello", "a/b", "nandu \241 \8364", "\119070"] $ \s -> roundTrip "Text" (T.pack s)
  forM_ ['a', '/', '\n', '\'', '\\', '\233', '\8364', '"'] $ \c -> roundTrip "Char" c
  roundTrip "[String]" (["a", "b/c", "", "d\"e"] :: [String])
  roundTrip "[Int]" ([1, 2, 3] :: [Int])
  roundTrip "[[String]]" ([["a"], ["b/c", ""]] :: [[String]])
  putStrLn "-- reading of the old (raw) format: console and URLs"
  des "hello//world/rest" >>= \r -> putStrLn ("  " ++ show (r :: Maybe (String, String)))
  des "a b/rest"         >>= \r -> putStrLn ("  " ++ show (r :: Maybe (String, String)))
  des "hello/rest"        >>= \r -> putStrLn ("  " ++ show (r :: Maybe (T.Text, String)))
  putStrLn "-- reading of JSON escapes"
  des "\"a\\u00e9\\n\\ud834\\udd1e\"/x" >>= \r -> putStrLn ("  " ++ show (r :: Maybe (String, String)))
  des "\"mal\\ud834\"/x" >>= \r -> putStrLn ("  lone surrogate: " ++ show (r :: Maybe (String, String)))
